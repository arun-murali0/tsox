#![cfg(feature = "incremental")]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use tsox::{
    IncrementalProject, ModuleGraph, ModuleResolver, ProjectConfig, ProjectReport, check_project,
};

// The acceptance tests of the query layer (LLD 6.5). Each row of the matrix says which
// units ran again, and every result is compared with a cold `check_project` over the same
// files on disk.

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str, files: &[(&str, &str)]) -> Self {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "incr-{name}-{}",
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let dir = Self(fs::canonicalize(&root).unwrap());
        dir.write(
            "tsconfig.json",
            r#"{ "compilerOptions": { "strict": true, "module": "esnext", "moduleResolution": "bundler" }, "include": ["*.ts"] }"#,
        );
        for (file, text) in files {
            dir.write(file, text);
        }
        dir
    }

    fn write(&self, file: &str, text: &str) {
        fs::write(self.0.join(file), text).unwrap();
        // A rewrite inside one timestamp tick is invisible to the fingerprint; the real
        // callers have a watcher. Move the clock so a test edit is seen.
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(60);
        let handle = fs::OpenOptions::new()
            .write(true)
            .open(self.0.join(file))
            .unwrap();
        handle
            .set_times(fs::FileTimes::new().set_modified(later))
            .unwrap();
    }

    fn remove(&self, file: &str) {
        fs::remove_file(self.0.join(file)).unwrap();
    }

    fn config(&self) -> PathBuf {
        self.0.join("tsconfig.json")
    }

    fn open(&self) -> IncrementalProject {
        IncrementalProject::open(&self.config()).unwrap()
    }
}

// What check_project gives on the files as they are now.
fn cold(dir: &Dir) -> ProjectReport {
    let config = ProjectConfig::load(dir.config()).unwrap();
    let resolver =
        ModuleResolver::with_tsconfig(config.config_file()).with_node_next(config.uses_node_next());
    let entries: Vec<PathBuf> = config
        .project_entries(&resolver)
        .into_iter()
        .map(|path| fs::canonicalize(&path).unwrap_or(path))
        .collect();
    let graph = ModuleGraph::build(&entries, &resolver)
        .unwrap()
        .with_root(config.root().to_path_buf())
        .with_config(&config)
        .with_type_entries_loaded();
    check_project(&graph)
}

// A report as text, by file name so a different numbering of files cannot hide a difference.
fn show(report: &ProjectReport, names: &dyn Fn(u32) -> String) -> String {
    let mut lines: Vec<String> = report
        .files
        .iter()
        .map(|file| {
            format!(
                "{} {:?} reads={} incomplete={:?}",
                names(file.file_id.index()),
                file.outcome,
                file.reads.len(),
                file.incomplete_reasons
            )
        })
        .collect();
    lines.sort();
    format!(
        "{lines:#?} unsettled={} cancelled={}",
        report.unsettled.len(),
        report.cancelled
    )
}

fn assert_same_as_cold(project: &mut IncrementalProject, dir: &Dir) -> ProjectReport {
    let incremental = project.check().unwrap();
    let cold_report = cold(dir);
    let bundle = project.graph();
    let incremental_names = |index: u32| {
        bundle
            .module_graph()
            .files()
            .path(tsox::FileId::new(index))
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let config = ProjectConfig::load(dir.config()).unwrap();
    let resolver =
        ModuleResolver::with_tsconfig(config.config_file()).with_node_next(config.uses_node_next());
    let entries: Vec<PathBuf> = config
        .project_entries(&resolver)
        .into_iter()
        .map(|path| fs::canonicalize(&path).unwrap_or(path))
        .collect();
    let graph = ModuleGraph::build(&entries, &resolver).unwrap();
    let cold_names = |index: u32| {
        graph
            .files()
            .path(tsox::FileId::new(index))
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    assert_eq!(
        show(&incremental, &incremental_names),
        show(&cold_report, &cold_names),
        "the incremental report differs from a cold check"
    );
    incremental
}

// The names of the units the checker ran for, sorted.
fn ran(project: &mut IncrementalProject) -> Vec<String> {
    let mut units: Vec<String> = project
        .take_log()
        .into_iter()
        .filter_map(|entry| entry.strip_prefix("check_scc:").map(str::to_owned))
        .collect();
    units.sort();
    units
}

const A: &str = "export interface A { x: number }\n\
export interface B { y: string }\n\
export function fa(): A { return { x: 1 }; }\n";
const B: &str = "import { A, fa } from './a';\n\
export const b: A = fa();\n\
export function fb(): number { return b.x; }\n";
const C: &str = "import { fb } from './b';\nexport const c: number = fb();\n";
const D: &str = "import { B } from './a';\nexport const d: B = { y: 's' };\n";
const E: &str = "export const e: number = 1;\n";

fn chain() -> Dir {
    Dir::new(
        "chain",
        &[
            ("a.ts", A),
            ("b.ts", B),
            ("c.ts", C),
            ("d.ts", D),
            ("e.ts", E),
        ],
    )
}

#[test]
fn a_first_check_runs_every_unit_and_equals_a_cold_check() {
    let dir = chain();
    let mut project = dir.open();
    assert_same_as_cold(&mut project, &dir);
    assert_eq!(ran(&mut project), ["a.ts", "b.ts", "c.ts", "d.ts", "e.ts"]);
}

// Row 1: unchanged save.
#[test]
fn an_unchanged_save_runs_nothing() {
    let dir = chain();
    let mut project = dir.open();
    project.check().unwrap();
    project.take_log();
    dir.write("a.ts", A); // same bytes, new timestamp
    assert_same_as_cold(&mut project, &dir);
    let log = project.take_log();
    assert!(
        log.iter().all(|entry| !entry.starts_with("check_scc")),
        "{log:?}"
    );
    assert!(!log.iter().any(|entry| entry == "module_header"), "{log:?}");
    assert_eq!(project.stats().units_checked, 0);
    assert_eq!(project.epoch(), 0);
}

// Row 2: whitespace or comment edit.
#[test]
fn a_whitespace_edit_runs_that_unit_and_stops_there() {
    let dir = chain();
    let mut project = dir.open();
    project.check().unwrap();
    project.take_log();
    dir.write(
        "a.ts",
        &format!("// a comment\n{}\n\n", A.replace(": ", " :  ")),
    );
    assert_same_as_cold(&mut project, &dir);
    let log = project.take_log();
    assert_eq!(
        log.iter()
            .filter(|e| e.starts_with("check_scc"))
            .collect::<Vec<_>>(),
        ["check_scc:a.ts"],
        "{log:?}"
    );
    assert_eq!(project.epoch(), 0, "the graph was kept");
}

// Row 3: body change, same signature.
#[test]
fn a_body_change_with_the_same_signature_leaves_importers_alone() {
    let dir = chain();
    let mut project = dir.open();
    project.check().unwrap();
    project.take_log();
    dir.write("a.ts", &A.replace("{ x: 1 }", "{ x: 2 }"));
    assert_same_as_cold(&mut project, &dir);
    assert_eq!(ran(&mut project), ["a.ts"]);
}

// Row 4: exported signature change.
#[test]
fn a_signature_change_reruns_only_the_importers_that_read_it() {
    let dir = chain();
    let mut project = dir.open();
    project.check().unwrap();
    project.take_log();
    dir.write(
        "a.ts",
        &A.replace("{ x: number }", "{ x: number; z?: number }"),
    );
    assert_same_as_cold(&mut project, &dir);
    // b reads A; d reads only B; c reads fb from b, whose own signature did not change.
    assert_eq!(ran(&mut project), ["a.ts", "b.ts"]);
}

// Row 5: another export of the same file changed, the one read did not.
#[test]
fn an_importer_of_an_unchanged_declaration_stays_green() {
    let dir = chain();
    let mut project = dir.open();
    project.check().unwrap();
    project.take_log();
    dir.write(
        "a.ts",
        &A.replace("{ y: string }", "{ y: string; w?: string }"),
    );
    assert_same_as_cold(&mut project, &dir);
    assert_eq!(ran(&mut project), ["a.ts", "d.ts"]);
}

// Row 6: an import added.
#[test]
fn an_added_import_moves_the_epoch_and_rebuilds_the_graph() {
    let dir = chain();
    let mut project = dir.open();
    project.check().unwrap();
    project.take_log();
    dir.write(
        "e.ts",
        "import { A } from './a';\nexport const e: A = { x: 1 };\n",
    );
    assert_same_as_cold(&mut project, &dir);
    assert_eq!(project.epoch(), 1);
    assert!(project.stats().rebuilt);
    let log = project.take_log();
    assert!(log.iter().any(|entry| entry == "module_header"), "{log:?}");
}

// Row 7: a dependency deep in the chain changes shape.
#[test]
fn a_shape_change_runs_along_the_chain_until_the_output_is_equal() {
    let dir = chain();
    let mut project = dir.open();
    let first = project.check().unwrap();
    assert_eq!(first.diagnostic_count(), 0);
    project.take_log();
    dir.write(
        "b.ts",
        &B.replace("fb(): number", "fb(): string")
            .replace("return b.x", "return 's'"),
    );
    let report = assert_same_as_cold(&mut project, &dir);
    assert_eq!(ran(&mut project), ["b.ts", "c.ts"]);
    assert_eq!(
        report.diagnostic_count(),
        1,
        "c now assigns a string to a number"
    );
}

// Row 8: a change inside a cycle.
#[test]
fn a_change_inside_a_cycle_reruns_the_whole_cycle_and_nothing_else() {
    let dir = Dir::new(
        "cycle",
        &[
            (
                "x.ts",
                "import { Y } from './y';\nexport interface X { y?: Y }\nexport const n: number = 1;\n",
            ),
            (
                "y.ts",
                "import { X } from './x';\nexport interface Y { x?: X }\n",
            ),
            (
                "z.ts",
                "import { X } from './x';\nexport const z: X = {};\n",
            ),
            ("e.ts", E),
        ],
    );
    let mut project = dir.open();
    assert_same_as_cold(&mut project, &dir);
    project.take_log();
    dir.write(
        "x.ts",
        "import { Y } from './y';\nexport interface X { y?: Y }\nexport const n: number = 2;\n",
    );
    assert_same_as_cold(&mut project, &dir);
    assert_eq!(
        ran(&mut project),
        ["x.ts"],
        "one unit holds x and y; z reads only X"
    );
}

// Row 9: a file added, removed.
#[test]
fn a_file_added_or_removed_moves_the_epoch() {
    let dir = chain();
    let mut project = dir.open();
    project.check().unwrap();
    dir.write("f.ts", "export const f: number = 'no';\n");
    project.rebuild().unwrap();
    let report = assert_same_as_cold(&mut project, &dir);
    assert_eq!(project.epoch(), 1);
    assert_eq!(report.diagnostic_count(), 1);
    dir.remove("f.ts");
    project.rebuild().unwrap();
    let report = assert_same_as_cold(&mut project, &dir);
    assert_eq!(project.epoch(), 2);
    assert_eq!(report.diagnostic_count(), 0);
}

// Row 10 (configuration): a compiler option the checker reads.
#[test]
fn a_config_change_rebuilds_and_applies_the_option() {
    let dir = Dir::new(
        "config",
        &[(
            "a.ts",
            "export function f(xs: number[]) { const n: number = xs[0]; return n; }\n",
        )],
    );
    let mut project = dir.open();
    let before = assert_same_as_cold(&mut project, &dir);
    assert_eq!(
        before.diagnostic_count(),
        1,
        "noUncheckedIndexedAccess is on by default"
    );
    dir.write(
        "tsconfig.json",
        r#"{ "compilerOptions": { "strict": true, "noUncheckedIndexedAccess": false, "module": "esnext", "moduleResolution": "bundler" }, "include": ["*.ts"] }"#,
    );
    let after = assert_same_as_cold(&mut project, &dir);
    assert_eq!(after.diagnostic_count(), 0);
    assert_eq!(project.epoch(), 1);
}

// Row 11: the global scope. Files that declare or use globals are checked by the full
// second pass; the answer must still equal a cold run.
#[test]
fn a_global_declaration_edit_gives_the_cold_answer() {
    let dir = Dir::new(
        "global",
        &[
            ("g.ts", "interface G { a: number }\n"),
            ("use.ts", "export const g: G = { a: 1 };\n"),
            ("e.ts", E),
        ],
    );
    let mut project = dir.open();
    assert_eq!(
        assert_same_as_cold(&mut project, &dir).diagnostic_count(),
        0
    );
    dir.write("g.ts", "interface G { a: string }\n");
    assert_eq!(
        assert_same_as_cold(&mut project, &dir).diagnostic_count(),
        1
    );
    dir.write("g.ts", "interface G { a: number }\n");
    assert_eq!(
        assert_same_as_cold(&mut project, &dir).diagnostic_count(),
        0
    );
}

// Row 12: an incomplete unit is never trusted: a file that fails to parse is reported, and
// fixing it reruns it and the files that read it.
#[test]
fn a_file_that_does_not_parse_is_reported_and_fixed_by_the_next_edit() {
    let dir = chain();
    let mut project = dir.open();
    project.check().unwrap();
    dir.write("a.ts", "export interface A { x: number\n");
    let broken = assert_same_as_cold(&mut project, &dir);
    assert!(broken.failure_count() > 0 || broken.diagnostic_count() > 0);
    dir.write("a.ts", A);
    let fixed = assert_same_as_cold(&mut project, &dir);
    assert_eq!(fixed.diagnostic_count(), 0);
}

#[test]
fn the_memo_can_be_bounded_without_changing_a_result() {
    let dir = chain();
    let mut project = dir.open();
    project.set_unit_cache_capacity(2);
    assert_same_as_cold(&mut project, &dir);
    dir.write(
        "a.ts",
        &A.replace("{ x: number }", "{ x: number; z?: number }"),
    );
    assert_same_as_cold(&mut project, &dir);
    project.evict();
    dir.write("e.ts", "export const e: string = 1;\n");
    assert_eq!(
        assert_same_as_cold(&mut project, &dir).diagnostic_count(),
        1
    );
}

#[test]
fn per_file_reports_come_from_the_projection_query() {
    let dir = chain();
    let mut project = dir.open();
    project.check().unwrap();
    let report = project
        .file_report(&dir.0.join("c.ts"))
        .expect("c is in the project");
    assert!(matches!(report.outcome, tsox::FileOutcome::Checked(_)));
}

// A run of random edits: after each one, the incremental report equals a cold check.
#[test]
fn random_edits_always_equal_a_cold_check() {
    let names = ["a", "b", "c", "d", "e", "f", "g", "h"];
    let mut files: Vec<(String, String)> = Vec::new();
    for (index, name) in names.iter().enumerate() {
        let mut text = String::new();
        for back in 1..=2usize {
            if index >= back {
                let dep = names[index - back];
                text += &format!("import {{ T_{dep}, f_{dep} }} from './{dep}';\n");
            }
        }
        text += &format!("export interface T_{name} {{ id: number }}\n");
        text += &format!("export function f_{name}(): T_{name} {{ return {{ id: {index} }}; }}\n");
        files.push((format!("{name}.ts"), text));
    }
    let dir = Dir::new(
        "random",
        &files
            .iter()
            .map(|(n, t)| (n.as_str(), t.as_str()))
            .collect::<Vec<_>>(),
    );
    let mut project = dir.open();
    assert_same_as_cold(&mut project, &dir);
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for round in 0..40 {
        let pick = (next() % names.len() as u64) as usize;
        let name = names[pick];
        let text = &mut files[pick].1;
        match next() % 5 {
            // a body edit
            0 => *text = text.replace(&format!("id: {pick}"), &format!("id: {round}")),
            // a signature edit
            1 => {
                *text = if text.contains("id: number;") {
                    text.replace("{ id: number; z: string }", "{ id: number }")
                } else {
                    text.replace("{ id: number }", "{ id: number; z: string }")
                }
            }
            // a comment
            2 => text.push_str(&format!("// round {round}\n")),
            // a type error
            3 => {
                *text = text.replace("export const bad", "const bad");
                text.push_str(&format!("export const bad: number = 's{round}';\n"));
            }
            // an import of another file
            _ => {
                let other = names[(next() % names.len() as u64) as usize];
                if other != name && !text.contains(&format!("from './{other}'")) {
                    let line = format!("import {{ T_{other} }} from './{other}';\n");
                    text.insert_str(0, &line);
                }
            }
        }
        dir.write(&files[pick].0, &files[pick].1);
        assert_same_as_cold(&mut project, &dir);
    }
}

// An editor buffer wins over the file, is checked like an edit, and gives what the same text
// saved to disk would give.
#[test]
fn a_buffer_body_edit_runs_one_unit_and_equals_the_saved_file() {
    let dir = chain();
    let mut project = dir.open();
    project.check().unwrap();
    project.take_log();
    let edited = A.replace("{ x: 1 }", "{ x: 2 }");
    project.set_buffer(&dir.0.join("a.ts"), &edited);
    project.check().unwrap();
    assert_eq!(ran(&mut project), ["a.ts"]);
    assert_eq!(project.epoch(), 0);
    // The disk still holds the old text; the buffer is what was checked. Saving it changes nothing.
    dir.write("a.ts", &edited);
    let report = assert_same_as_cold(&mut project, &dir);
    assert_eq!(ran(&mut project), Vec::<String>::new());
    drop(report);
}

#[test]
fn a_buffer_signature_edit_reports_in_the_importers_and_closing_it_restores_the_file() {
    let dir = chain();
    let mut project = dir.open();
    let before = project.check().unwrap();
    let before_errors: usize = before.files.iter().map(|f| diagnostics(f)).sum();
    project.set_buffer(
        &dir.0.join("a.ts"),
        &A.replace("{ x: number }", "{ x: string }"),
    );
    let during = project.check().unwrap();
    let during_errors: usize = during.files.iter().map(|f| diagnostics(f)).sum();
    assert!(
        during_errors > before_errors,
        "{before_errors} -> {during_errors}"
    );
    project.close_buffer(&dir.0.join("a.ts"));
    assert_same_as_cold(&mut project, &dir);
}

#[test]
fn a_buffer_that_adds_an_import_rebuilds_the_graph_from_the_buffer() {
    let dir = chain();
    let mut project = dir.open();
    project.check().unwrap();
    let text = "import { A } from './a';\nexport const e: A = { x: 1 };\n";
    project.set_buffer(&dir.0.join("e.ts"), text);
    project.check().unwrap();
    assert_eq!(project.epoch(), 1);
    assert!(project.stats().rebuilt);
    dir.write("e.ts", text);
    assert_same_as_cold(&mut project, &dir);
}

fn diagnostics(file: &tsox::FileReport) -> usize {
    match &file.outcome {
        tsox::FileOutcome::Checked(result) => result.len(),
        _ => 0,
    }
}

#[test]
fn a_cancelled_check_is_an_error_and_the_next_check_is_exact() {
    let dir = chain();
    let mut project = dir.open();
    let token = tsox::CancelToken::new();
    token.cancel();
    assert!(project.check_cancellable(&token).is_err());
    assert_same_as_cold(&mut project, &dir);
}

#[test]
fn a_check_cancelled_while_running_never_leaves_a_partial_memo() {
    let files: Vec<(String, String)> = (0..400)
        .map(|i| {
            let text = if i == 0 {
                "export const v0: number = 0;\n".to_owned()
            } else {
                format!(
                    "import {{ v{p} }} from './f{p}';\nexport const v{i}: number = v{p} + 1;\n",
                    p = i - 1
                )
            };
            (format!("f{i}.ts"), text)
        })
        .collect();
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let dir = Dir::new("cancel", &refs);
    let mut project = dir.open();
    for delay_ms in [0u64, 1, 3, 8] {
        let token = tsox::CancelToken::new();
        let stopper = token.clone();
        let thread = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
            stopper.cancel();
        });
        let _ = project.check_cancellable(&token);
        thread.join().unwrap();
        // Whatever stopped, the next check is complete and equals a cold one.
        assert_same_as_cold(&mut project, &dir);
        dir.write("f0.ts", &format!("export const v0: number = {delay_ms};\n"));
    }
}

// A rebuilt graph keeps the memo of every unit whose plan is the same: an import added to one
// file runs that file and what reads it, not the project.
#[test]
fn an_added_import_after_a_rebuild_runs_only_the_file_that_changed() {
    let dir = chain();
    let mut project = dir.open();
    project.check().unwrap();
    project.take_log();
    dir.write(
        "e.ts",
        "import { A } from './a';\nexport const e: A = { x: 1 };\n",
    );
    assert_same_as_cold(&mut project, &dir);
    assert!(project.stats().rebuilt);
    assert_eq!(ran(&mut project), ["e.ts"]);
    assert_eq!(project.stats().units_reused, 4);
}

// A file that sorts after the others moves no file id, so nothing else runs.
#[test]
fn a_file_added_at_the_end_of_the_path_order_runs_alone() {
    let dir = chain();
    let mut project = dir.open();
    project.check().unwrap();
    project.take_log();
    dir.write(
        "z.ts",
        "import { fa } from './a';\nexport const z = fa();\n",
    );
    project.rebuild().unwrap();
    assert_same_as_cold(&mut project, &dir);
    assert_eq!(ran(&mut project), ["z.ts"]);
    // Removing it again leaves the rest as they were.
    dir.remove("z.ts");
    project.rebuild().unwrap();
    assert_same_as_cold(&mut project, &dir);
    assert_eq!(ran(&mut project), Vec::<String>::new());
}

// A file that sorts first renumbers the others: every unit that sees an id move runs again,
// and the answer is still the cold one.
#[test]
fn a_file_added_at_the_front_renumbers_and_is_still_exact() {
    let dir = chain();
    let mut project = dir.open();
    project.check().unwrap();
    dir.write("0.ts", "export const zero: number = 0;\n");
    project.rebuild().unwrap();
    assert_same_as_cold(&mut project, &dir);
    dir.write("c.ts", &format!("import {{ zero }} from './0';\n{C}"));
    assert_same_as_cold(&mut project, &dir);
}

// Files added, removed and rewired at random, with a rebuild each time: always the cold answer.
#[test]
fn random_graph_edits_always_equal_a_cold_check() {
    let names = ["a", "b", "c", "d", "e", "f"];
    let make = |name: &str, deps: &[&str], tag: usize| {
        let mut text = String::new();
        for dep in deps {
            text += &format!("import {{ T_{dep} }} from './{dep}';\n");
        }
        text += &format!("export interface T_{name} {{ id: number }}\n");
        text += &format!("export const v_{name}: T_{name} = {{ id: {tag} }};\n");
        text
    };
    let files: Vec<(String, String)> = names
        .iter()
        .take(3)
        .map(|n| (format!("{n}.ts"), make(n, &[], 0)))
        .collect();
    let dir = Dir::new(
        "graph-fuzz",
        &files
            .iter()
            .map(|(n, t)| (n.as_str(), t.as_str()))
            .collect::<Vec<_>>(),
    );
    let mut project = dir.open();
    assert_same_as_cold(&mut project, &dir);
    let mut present: Vec<bool> = names.iter().enumerate().map(|(i, _)| i < 3).collect();
    let mut state = std::env::var("TSOX_FUZZ_SEED")
        .ok()
        .and_then(|seed| seed.parse::<u64>().ok())
        .unwrap_or(0x9e37_79b9_7f4a_7c15u64);
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for round in 0..60 {
        let pick = (next() % names.len() as u64) as usize;
        let name = names[pick];
        match next() % 4 {
            0 if present.iter().filter(|p| **p).count() > 1 => {
                if present[pick] {
                    dir.remove(&format!("{name}.ts"));
                    present[pick] = false;
                }
            }
            1 => {
                dir.write(&format!("{name}.ts"), &make(name, &[], round));
                present[pick] = true;
            }
            _ => {
                let others: Vec<&str> = names
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| present[*i] && *i != pick)
                    .map(|(_, n)| *n)
                    .filter(|_| next() % 2 == 0)
                    .collect();
                dir.write(&format!("{name}.ts"), &make(name, &others, round));
                present[pick] = true;
            }
        }
        // Importers of a removed file fail to resolve it, which is also what a cold check says.
        project.rebuild().unwrap();
        assert_same_as_cold(&mut project, &dir);
    }
}

// Ambient providers (declaration files with `declare module`) are checked before the units and
// answered from their own memos; every unit is keyed by what they declared.
const PROVIDER: &str =
    "declare module 'virtual' { export const v: number; export function f(x: number): string; }\n";
const USES_A: &str = "import { v, f } from 'virtual';\nexport const a: number = v;\nexport const s: string = f(a);\n";
const USES_B: &str = "import { v } from 'virtual';\nexport const b: string = v;\n"; // an error
const PLAIN: &str = "export const p: number = 1;\n";

fn ambient() -> Dir {
    Dir::new(
        "ambient",
        &[
            ("types.d.ts", PROVIDER),
            ("one.ts", USES_A),
            ("two.ts", USES_B),
            ("plain.ts", PLAIN),
        ],
    )
}

fn providers_ran(project: &mut IncrementalProject) -> Vec<String> {
    let mut found: Vec<String> = project
        .take_log()
        .into_iter()
        .filter_map(|entry| entry.strip_prefix("check_provider:").map(str::to_owned))
        .collect();
    found.sort();
    found
}

#[test]
fn a_project_with_ambient_providers_is_incremental_and_equals_a_cold_check() {
    let dir = ambient();
    let mut project = dir.open();
    let report = assert_same_as_cold(&mut project, &dir);
    assert!(
        report.diagnostic_count() >= 1,
        "two.ts assigns a number to a string"
    );
    assert_eq!(project.stats().ambient_providers, 1);
    assert_eq!(project.stats().providers_checked, 1);
    assert_eq!(providers_ran(&mut project), ["types.d.ts"]);

    // Nothing changed: the provider and every unit come from their memos.
    assert_same_as_cold(&mut project, &dir);
    assert_eq!(project.stats().units_checked, 0);
    assert_eq!(project.stats().providers_checked, 0);
    // The provider's own unit is checked before the pass and never asked of the memo, so it is
    // in neither count: what was asked is all reused, and the provider is the difference.
    assert_eq!(project.stats().units_reused, project.stats().units_total);
    assert_eq!(
        project.stats().units_total + project.stats().ambient_providers,
        project.graph().module_graph().sccs().len()
    );
    project.take_log();

    // A body edit in a unit that does not touch the provider: that unit only, no provider.
    dir.write("plain.ts", "export const p: number = 2;\n");
    assert_same_as_cold(&mut project, &dir);
    assert_eq!(ran(&mut project), ["plain.ts"]);
    assert_eq!(project.stats().providers_checked, 0);
}

#[test]
fn an_edit_to_what_a_provider_declares_reruns_the_units_and_stays_exact() {
    let dir = ambient();
    let mut project = dir.open();
    project.check().unwrap();
    project.take_log();
    // The declared type changes: every unit sees a different environment.
    dir.write("types.d.ts", &PROVIDER.replace("v: number", "v: string"));
    assert_same_as_cold(&mut project, &dir);
    assert_eq!(project.stats().providers_checked, 1);
    assert!(project.stats().units_checked >= 2, "{:?}", project.stats());
    // A comment in the provider declares nothing new: it is checked, the units are not.
    project.take_log();
    dir.write(
        "types.d.ts",
        &format!("{}// note\n", PROVIDER.replace("v: number", "v: string")),
    );
    assert_same_as_cold(&mut project, &dir);
    assert_eq!(project.stats().providers_checked, 1);
    assert_eq!(project.stats().units_checked, 0, "{:?}", project.stats());
}

#[test]
fn a_provider_added_or_removed_is_still_the_cold_answer() {
    let dir = Dir::new("ambient-add", &[("one.ts", USES_A), ("plain.ts", PLAIN)]);
    let mut project = dir.open();
    assert_same_as_cold(&mut project, &dir); // 'virtual' is unresolved
    dir.write("types.d.ts", PROVIDER);
    project.rebuild().unwrap();
    assert_same_as_cold(&mut project, &dir);
    dir.remove("types.d.ts");
    project.rebuild().unwrap();
    assert_same_as_cold(&mut project, &dir);
}

#[test]
fn random_edits_with_providers_always_equal_a_cold_check() {
    let dir = ambient();
    let mut project = dir.open();
    assert_same_as_cold(&mut project, &dir);
    let mut state = std::env::var("TSOX_FUZZ_SEED")
        .ok()
        .and_then(|seed| seed.parse::<u64>().ok())
        .unwrap_or(0x1234_5678_9abc_def1u64);
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for round in 0..40 {
        match next() % 6 {
            0 => dir.write(
                "types.d.ts",
                &PROVIDER.replace(
                    "v: number",
                    if round % 2 == 0 {
                        "v: string"
                    } else {
                        "v: number"
                    },
                ),
            ),
            1 => dir.write(
                "types.d.ts",
                &format!(
                    "{PROVIDER}declare module 'extra{}' {{ export const e: number }}\n",
                    round % 3
                ),
            ),
            2 => dir.write("one.ts", &format!("{USES_A}// {round}\n")),
            3 => dir.write(
                "two.ts",
                &USES_B.replace("string", if round % 2 == 0 { "number" } else { "string" }),
            ),
            4 => dir.write("plain.ts", &format!("export const p: number = {round};\n")),
            _ => dir.write(
                "plain.ts",
                &format!(
                    "import {{ e }} from 'extra{}';\nexport const p: number = e;\n",
                    round % 3
                ),
            ),
        }
        assert_same_as_cold(&mut project, &dir);
    }
}

// ---------------------------------------------------------------------------------------
// The watcher's condition, the work accounting, and what counts as a changed file.

// What `--watch` prints on: `changed` says the report may differ, not that a unit ran.
#[test]
fn the_watch_condition_follows_what_a_reader_can_see() {
    let dir = chain();
    let mut project = dir.open();
    project.check().unwrap();
    assert!(project.stats().changed, "the first report is news");

    project.check().unwrap();
    assert!(!project.stats().changed, "nothing happened");

    dir.write("a.ts", A); // same bytes, new timestamp
    project.check().unwrap();
    assert!(!project.stats().changed, "a touch is not a change");
    assert_eq!(project.stats().units_checked, 0);

    dir.write("a.ts", &format!("// a comment\n{A}"));
    project.check().unwrap();
    assert!(project.stats().changed, "an edit is a change");
    assert_eq!(project.stats().units_checked, 1);

    // A file nothing imports comes and goes: no unit of the others runs, and the project's
    // report still changed. The old test, "no unit checked and not rebuilt", was silent here
    // whenever the caller (the watcher) had rebuilt the graph itself before the check.
    dir.write("z.ts", "export const z: number = 'no';\n");
    project.rebuild().unwrap();
    let report = project.check().unwrap();
    assert_eq!(report.diagnostic_count(), 1);
    assert!(project.stats().rebuilt && project.stats().changed);
    dir.remove("z.ts");
    project.rebuild().unwrap();
    let report = project.check().unwrap();
    assert_eq!(report.diagnostic_count(), 0);
    assert_eq!(project.stats().units_checked, 0);
    assert!(
        project.stats().rebuilt && project.stats().changed,
        "{:?}",
        project.stats()
    );

    project.check().unwrap();
    assert!(!project.stats().changed);
}

// Every unit the driver asks for is checked or reused, once.
#[test]
fn work_accounting_counts_each_unit_once() {
    let dir = chain();
    let mut project = dir.open();
    project.check().unwrap();
    let stats = project.stats();
    assert_eq!((stats.units_checked, stats.units_reused), (5, 0));
    assert_eq!(stats.units_total, 5);

    project.check().unwrap();
    let stats = project.stats();
    assert_eq!((stats.units_checked, stats.units_reused), (0, 5));

    // A signature change reaches its importers; whatever runs, the three always add up.
    dir.write("a.ts", &A.replace("x: number", "x: string"));
    assert_same_as_cold(&mut project, &dir);
    let stats = project.stats();
    assert!(stats.units_checked >= 1, "{stats:?}");
    assert_eq!(stats.units_total, 5, "{stats:?}");
    assert_eq!(stats.units_checked + stats.units_reused, stats.units_total);
}

// A rewrite that keeps the length and lands in the same timestamp tick leaves the stat call
// unchanged. The fingerprint is trusted only for a file that was already old when it was read.
#[test]
fn a_same_length_rewrite_in_the_same_tick_is_seen() {
    let dir = chain();
    // Written now, not stamped a minute ahead as `Dir::write` does, so it is a recent file.
    fs::write(dir.0.join("e.ts"), "export const e: number = 1;\n").unwrap();
    let mut project = dir.open();
    project.check().unwrap();
    let path = dir.0.join("e.ts");
    let stamp = fs::metadata(&path).unwrap().modified().unwrap();

    fs::write(&path, "export const e: string = 1;\n").unwrap(); // same length, now a type error
    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(stamp)
        .unwrap();
    let report = assert_same_as_cold(&mut project, &dir);
    assert_eq!(report.diagnostic_count(), 1);
    assert_eq!(ran(&mut project), ["e.ts"]);
}

// A refresh whose rebuild failed has not seen the files it read. Recording them as seen
// anyway made the next refresh call them unchanged, and the project kept the old graph and
// the old results for a file that had moved on.
#[test]
fn a_failed_rebuild_does_not_lose_the_edit_that_asked_for_it() {
    let dir = chain();
    let mut project = dir.open();
    project.check().unwrap();

    let config = dir.config();
    let original = fs::read(&config).unwrap();
    let stamp = fs::metadata(&config).unwrap().modified().unwrap();
    fs::write(&config, "{ not json").unwrap();
    dir.write(
        "b.ts",
        &format!("import {{ d }} from './d';\n{B}export const bd = d;\n"),
    );
    assert!(project.check().is_err(), "the configuration does not load");

    // The configuration is back, byte for byte and with its old stamp: only b.ts can say
    // that the graph is out of date.
    fs::write(&config, &original).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(&config)
        .unwrap()
        .set_modified(stamp)
        .unwrap();
    assert_same_as_cold(&mut project, &dir);
    assert!(project.stats().rebuilt, "{:?}", project.stats());
}

// ---------------------------------------------------------------------------------------
// The second pass (globals, augmentations, ambient modules) is memoized by what it reads.

const GLOBAL_FN: &str = "declare function gf(x: number): string;\n";

fn globals() -> Dir {
    Dir::new(
        "second",
        &[
            ("g.ts", GLOBAL_FN),
            ("use.ts", "export const s: string = gf(1);\n"),
            ("e.ts", E),
        ],
    )
}

#[test]
fn an_unchanged_check_reuses_the_second_pass() {
    let dir = globals();
    let mut project = dir.open();
    assert_eq!(
        assert_same_as_cold(&mut project, &dir).diagnostic_count(),
        0
    );
    assert_eq!(project.stats().second_pass, tsox::SecondPass::Ran);

    project.check().unwrap();
    assert_eq!(project.stats().second_pass, tsox::SecondPass::Reused);
    assert!(!project.stats().changed);

    dir.write("use.ts", "export const s: string = gf(1);\n"); // a touch
    assert_same_as_cold(&mut project, &dir);
    assert_eq!(project.stats().second_pass, tsox::SecondPass::Reused);

    // What the second pass reads changed: it runs again, and the answer is the cold one.
    dir.write("g.ts", "declare function gf(x: number): number;\n");
    assert_eq!(
        assert_same_as_cold(&mut project, &dir).diagnostic_count(),
        1
    );
    assert_eq!(project.stats().second_pass, tsox::SecondPass::Ran);
}

// The case a fingerprint of the first pass's outputs alone would get wrong: the first pass
// cannot see `gf`, so it reports the same for `gf(1)` and `gf('x')`; only the second pass
// knows the call is wrong. The text of the file the second pass checks is part of its key.
#[test]
fn a_body_edit_that_only_the_second_pass_can_see_is_not_answered_from_the_old_result() {
    let dir = globals();
    let mut project = dir.open();
    assert_eq!(
        assert_same_as_cold(&mut project, &dir).diagnostic_count(),
        0
    );
    dir.write("use.ts", "export const s: string = gf('x');\n");
    assert_eq!(
        assert_same_as_cold(&mut project, &dir).diagnostic_count(),
        1
    );
    assert_eq!(project.stats().second_pass, tsox::SecondPass::Ran);
    dir.write("use.ts", "export const s: string = gf(1);\n");
    assert_eq!(
        assert_same_as_cold(&mut project, &dir).diagnostic_count(),
        0
    );
}

// Random edits to a project whose answer depends on the second pass: always the cold answer.
#[test]
fn random_edits_with_globals_always_equal_a_cold_check() {
    let g = [
        GLOBAL_FN,
        "declare function gf(x: string): string;\n",
        "declare function gf(x: number): number;\n",
        "interface G { a: number }\n",
    ];
    let u = [
        "export const s: string = gf(1);\n",
        "export const s: string = gf('x');\n",
        "export const n: number = gf(2);\n",
        "export const g: G = { a: 1 };\n",
        "export const z = 1;\n",
    ];
    let e = [
        E,
        "export const e: string = 1;\n",
        "// c\nexport const e: number = 1;\n",
    ];
    let dir = globals();
    let mut project = dir.open();
    assert_same_as_cold(&mut project, &dir);
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = move |bound: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % bound as u64) as usize
    };
    for _ in 0..40 {
        match next(4) {
            0 => dir.write("g.ts", g[next(g.len())]),
            1 => dir.write("use.ts", u[next(u.len())]),
            2 => dir.write("e.ts", e[next(e.len())]),
            _ => {} // no edit at all: the kept second pass answers
        }
        assert_same_as_cold(&mut project, &dir);
    }
}

// A module augmentation reaches a file that imports the augmented module only through
// another file. Pinned against the cold answer through every edit of the augmentation.
#[test]
fn an_augmentation_reaches_a_transitive_importer() {
    let aug = [
        "export {};\ndeclare module './a' { interface A { extra: number } }\n",
        "export {};\ndeclare module './a' { interface A { extra: string } }\n",
        "export {};\n",
    ];
    let dir = Dir::new(
        "transitive",
        &[
            ("a.ts", "export interface A { x: number }\n"),
            (
                "b.ts",
                "import { A } from './a';\nexport const b = (a: A): A => a;\n",
            ),
            (
                "c.ts",
                "import { b } from './b';\nimport { A } from './a';\nexport const n: number = b({ x: 1 } as A).extra;\n",
            ),
            ("aug.ts", aug[0]),
        ],
    );
    let mut project = dir.open();
    assert_same_as_cold(&mut project, &dir);
    for step in [1, 2, 0, 1, 0, 2] {
        dir.write("aug.ts", aug[step]);
        assert_same_as_cold(&mut project, &dir);
    }
}
