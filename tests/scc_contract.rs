#![cfg(feature = "module-resolution")]

use std::fs;
use std::path::{Path, PathBuf};

use tsox::{FileOutcome, ModuleGraph, ModuleResolver, ProjectReport, check_project_bounded};

// Problem: the doc for the cycle settlement says what a cycle must do, but only some of it
// was pinned by a test.
// Picked: a short test for each condition not covered in project_imports.rs.
// Now: the result of a run does not depend on the order the entries are given, an unsettled
// cycle does not hide an importer's own errors, and two unrelated cycles in one layer are
// settled on their own.

struct Run {
    graph: ModuleGraph,
    report: ProjectReport,
}

fn run(test: &str, files: &[(&str, &str)], entries_reversed: bool, rounds: usize) -> Run {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("scc-contract-{test}"));
    let _ = fs::remove_dir_all(&root);
    let mut paths: Vec<PathBuf> = Vec::new();
    for (name, source) in files {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, source).unwrap();
        paths.push(path);
    }
    if entries_reversed {
        paths.reverse();
    }
    let graph = ModuleGraph::build(&paths, &ModuleResolver::new())
        .expect("the project builds")
        .with_root(root);
    let report = check_project_bounded(&graph, Some(rounds));
    Run { graph, report }
}

fn name_of(run: &Run, id: tsox::FileId) -> String {
    let path = run.graph.files().path(id).unwrap();
    path.file_name().unwrap().to_string_lossy().into_owned()
}

impl Run {
    fn unsettled(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .report
            .unsettled
            .iter()
            .map(|id| name_of(self, *id))
            .collect();
        names.sort();
        names
    }

    // Every file's diagnostics as text, by file name, so two runs can be compared.
    fn diagnostics(&self) -> Vec<(String, Vec<String>)> {
        let mut all: Vec<(String, Vec<String>)> = self
            .report
            .files
            .iter()
            .map(|file| {
                let lines = match &file.outcome {
                    FileOutcome::Checked(diagnostics) => diagnostics
                        .iter()
                        .map(|d| format!("{}:{}:{}", d.start, d.end, d.message))
                        .collect(),
                    other => vec![format!("{other:?}")],
                };
                (name_of(self, file.file_id), lines)
            })
            .collect();
        all.sort();
        all
    }

    fn tsc_codes(&self, file: &str) -> Vec<u32> {
        let mut codes = Vec::new();
        for report in &self.report.files {
            if name_of(self, report.file_id) == file {
                if let FileOutcome::Checked(diagnostics) = &report.outcome {
                    codes.extend(diagnostics.iter().filter_map(|d| d.code.tsc_code()));
                }
            }
        }
        codes
    }
}

// A ring of `count` modules, each re-exporting the next with `export *`, where the last one
// declares `Deep`. `Deep` can only reach the first module by going once around the ring.
fn ring(prefix: &str, count: usize) -> Vec<(String, String)> {
    let mut sources = Vec::new();
    for index in 0..count {
        let next = (index + 1) % count;
        let mut source = format!("export * from './{prefix}{next}';");
        if index == count - 1 {
            source.push_str("\nexport interface Deep { value: number }");
        }
        sources.push((format!("{prefix}{index}.ts"), source));
    }
    sources
}

fn as_files(sources: &[(String, String)]) -> Vec<(&str, &str)> {
    sources
        .iter()
        .map(|(name, source)| (name.as_str(), source.as_str()))
        .collect()
}

#[test]
fn the_outcome_of_an_unsettled_cycle_does_not_depend_on_the_order_of_the_entries() {
    let mut sources = ring("m", 7);
    sources.push((
        "entry.ts".to_string(),
        "import { Deep } from './m0'; const value: Deep = { value: 'wrong' };".to_string(),
    ));
    let files = as_files(&sources);
    let forward = run("order-forward", &files, false, 2);
    let backward = run("order-backward", &files, true, 2);
    assert!(
        !forward.unsettled().is_empty(),
        "the ring should run out of rounds"
    );
    assert_eq!(forward.unsettled(), backward.unsettled());
    assert_eq!(forward.diagnostics(), backward.diagnostics());
}

#[test]
fn an_unsettled_cycle_does_not_hide_the_errors_of_the_file_that_imports_it() {
    // The importer stays quiet about a name it cannot find in a partial list, and about
    // nothing else.
    let mut sources = ring("m", 7);
    sources.push((
        "entry.ts".to_string(),
        "import { Deep } from './m0';\nconst own: number = 'text';\nexport type Keep = Deep;"
            .to_string(),
    ));
    let result = run("importer-keeps-its-errors", &as_files(&sources), false, 2);
    assert!(
        !result.unsettled().is_empty(),
        "the ring should run out of rounds"
    );
    assert!(
        result.tsc_codes("entry.ts").contains(&2322),
        "the importer's own type error must still be reported: {:?}",
        result.diagnostics()
    );
}

#[test]
fn two_unrelated_cycles_in_one_layer_are_settled_on_their_own() {
    // A cycle that settles is not listed as unsettled because a different cycle in the same
    // layer ran out of rounds. Each cycle has its own bound and its own failure.
    let mut sources = ring("long", 7);
    sources.extend([
        (
            "small0.ts".to_string(),
            "export * from './small1';".to_string(),
        ),
        (
            "small1.ts".to_string(),
            "export * from './small0';\nexport const ready = 1;".to_string(),
        ),
        (
            "use_small.ts".to_string(),
            "import { ready } from './small0';\nconst n: string = ready;".to_string(),
        ),
    ]);
    let result = run("two-cycles", &as_files(&sources), false, 4);
    let unsettled = result.unsettled();
    assert!(
        unsettled.iter().any(|name| name.starts_with("long")),
        "{unsettled:?}"
    );
    assert!(
        !unsettled.iter().any(|name| name.starts_with("small")),
        "the small cycle settles and must not be listed: {unsettled:?}"
    );
    // Its exports are complete, so an importer is told about a real mismatch.
    assert!(
        result.tsc_codes("use_small.ts").contains(&2322),
        "{:?}",
        result.diagnostics()
    );
}

fn reasons_of(result: &Run, file: &str) -> Vec<tsox::IncompleteReason> {
    result
        .report
        .files
        .iter()
        .filter(|report| name_of(result, report.file_id) == file)
        .flat_map(|report| report.incomplete_reasons.clone())
        .collect()
}

#[test]
fn a_cycle_that_does_not_settle_names_its_files_as_incomplete() {
    let sources = ring("m", 7);
    let result = run("unsettled-named", &as_files(&sources), false, 2);
    for index in 0..7 {
        let reasons = reasons_of(&result, &format!("m{index}.ts"));
        assert!(
            reasons.contains(&tsox::IncompleteReason::CycleDidNotSettle),
            "m{index}.ts: {reasons:?}"
        );
    }
}

#[test]
fn a_cycle_that_settles_is_not_named_as_incomplete() {
    let sources = ring("m", 7);
    let result = run("settled-not-named", &as_files(&sources), false, 20);
    assert!(result.unsettled().is_empty());
    for index in 0..7 {
        let reasons = reasons_of(&result, &format!("m{index}.ts"));
        assert!(
            !reasons.contains(&tsox::IncompleteReason::CycleDidNotSettle),
            "m{index}.ts: {reasons:?}"
        );
    }
}

#[test]
fn an_unsettled_cycle_reports_no_missing_name_among_its_own_members() {
    // m0 uses a name that only reaches m3 after the ring has gone round. With two rounds
    // the ring has not settled, and the last round must not call the name missing.
    let mut sources = ring("m", 7);
    sources[0].1 =
        "export * from './m1';\nimport { Deep } from './m3';\nexport const ok: Deep = { value: 1 };"
            .to_string();
    let result = run("no-false-missing", &as_files(&sources), false, 2);
    assert!(!result.unsettled().is_empty());
    // A file of a cycle that did not settle was checked against a snapshot that had not
    // settled (see `ProjectReport::unsettled`), so it may still say that it cannot resolve a
    // type annotation yet. What it must never do is call a name or module missing.
    for (file, lines) in result.diagnostics() {
        for line in &lines {
            assert!(
                !line.contains("has no exported member")
                    && !line.contains("has no default export")
                    && !line.contains("Cannot find module"),
                "{file}: {line}"
            );
        }
    }
}
