#![cfg(feature = "module-resolution")]

use std::fs;
use std::path::{Path, PathBuf};

use tsox::{Diagnostic, FileOutcome, ModuleGraph, ModuleResolver, Severity, check_project};

// Step 7a: an import that cannot be bound says so. A relative specifier no file answers
// is TS2307, a name a complete module does not export is TS2305, and a default import of
// a module with no default is TS1192. Against a module whose export list left something
// out (a class, `export *`) a missing name may be one we dropped, so nothing is said.

struct Checked {
    graph: ModuleGraph,
    report: tsox::ProjectReport,
}

fn run(test: &str, files: &[(&str, &str)]) -> Checked {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("import-diagnostics-{test}"));
    let _ = fs::remove_dir_all(&root);
    let mut paths: Vec<PathBuf> = Vec::new();
    for (name, source) in files {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, source).unwrap();
        paths.push(path);
    }
    let graph = ModuleGraph::build(&paths, &ModuleResolver::new())
        .expect("the project builds")
        .with_root(root);
    let report = check_project(&graph);
    Checked { graph, report }
}

impl Checked {
    fn errors(&self, file: &str) -> Vec<&Diagnostic> {
        for report in &self.report.files {
            let path = self.graph.files().path(report.file_id).unwrap();
            if path.file_name().unwrap().to_string_lossy() == file {
                return match &report.outcome {
                    FileOutcome::Checked(d) => {
                        d.iter().filter(|d| d.severity == Severity::Error).collect()
                    }
                    other => panic!("{file} was not checked: {other:?}"),
                };
            }
        }
        panic!("{file} is not in the report");
    }

    fn codes(&self, file: &str) -> Vec<Option<u32>> {
        self.errors(file)
            .iter()
            .map(|d| d.code.tsc_code())
            .collect()
    }
}

#[test]
fn a_relative_import_of_a_missing_file_is_ts2307_at_the_specifier() {
    let source = "import { X } from './nope';\nlet y = 1;";
    let project = run("missing-module", &[("b.ts", source)]);
    let errors = project.errors("b.ts");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].code.tsc_code(), Some(2307));
    assert!(errors[0].message.contains("'./nope'"));
    assert_eq!(
        &source[errors[0].start as usize..errors[0].end as usize],
        "'./nope'"
    );
}

// Whether something above the test directory could answer a package specifier for
// TypeScript (an `@types` directory, or a config with `paths`), which makes the checker
// leave package specifiers alone. Tests that expect a report only assert it without one.
fn something_above_may_answer_package_specifiers(project: &Path) -> bool {
    project.ancestors().any(|dir| {
        dir.join("node_modules").join("@types").is_dir()
            || ["tsconfig.json", "jsconfig.json"].iter().any(|name| {
                fs::read_to_string(dir.join(name)).is_ok_and(|text| {
                    [
                        "\"paths\"",
                        "\"baseUrl\"",
                        "\"rootDirs\"",
                        "\"extends\"",
                        "\"typeRoots\"",
                    ]
                    .iter()
                    .any(|key| text.contains(key))
                })
            })
    })
}

#[test]
fn a_package_specifier_nothing_answers_is_ts2307_when_nothing_could_hide_it() {
    let project = run(
        "bare",
        &[("b.ts", "import { X } from 'left-pad'; let y = 1;")],
    );
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("import-diagnostics-bare");
    if something_above_may_answer_package_specifiers(&root) {
        return;
    }
    let codes: Vec<_> = project
        .errors("b.ts")
        .iter()
        .map(|d| d.code.tsc_code())
        .collect();
    assert_eq!(codes, vec![Some(2307)], "{:?}", project.errors("b.ts"));
}

#[test]
fn a_package_specifier_is_left_alone_when_the_config_has_paths() {
    // `paths` can map `left-pad` to a file the checker does not read, so it is not called
    // missing.
    let project = run(
        "bare-paths",
        &[
            (
                "tsconfig.json",
                r#"{ "compilerOptions": { "paths": { "left-pad": ["./x"] } } }"#,
            ),
            ("b.ts", "import { X } from 'left-pad'; let y = 1;"),
        ],
    );
    assert!(
        project.errors("b.ts").is_empty(),
        "{:?}",
        project.errors("b.ts")
    );
}

#[test]
fn a_node_builtin_is_never_called_a_missing_module() {
    let project = run(
        "builtin",
        &[(
            "b.ts",
            "import { readFileSync } from 'fs'; import 'node:path';",
        )],
    );
    assert!(
        project.errors("b.ts").is_empty(),
        "{:?}",
        project.errors("b.ts")
    );
}

#[test]
fn a_misspelled_import_name_is_ts2305_at_the_name() {
    let source = "import { Usr } from './a';\nlet y = 1;";
    let project = run(
        "typo",
        &[
            ("a.ts", "export interface User { name: string }"),
            ("b.ts", source),
        ],
    );
    let errors = project.errors("b.ts");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].code.tsc_code(), Some(2305));
    assert!(errors[0].message.contains("has no exported member 'Usr'"));
    assert_eq!(
        &source[errors[0].start as usize..errors[0].end as usize],
        "Usr"
    );
}

#[test]
fn an_aliased_import_is_reported_at_the_imported_name_not_the_alias() {
    let source = "import { Usr as U } from './a';";
    let project = run(
        "alias",
        &[
            ("a.ts", "export interface User { name: string }"),
            ("b.ts", source),
        ],
    );
    let errors = project.errors("b.ts");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(
        &source[errors[0].start as usize..errors[0].end as usize],
        "Usr"
    );
}

#[test]
fn a_default_import_of_a_module_without_one_is_ts1192() {
    let source = "import a from './a';";
    let project = run(
        "no-default",
        &[
            ("a.ts", "export interface User { name: string }"),
            ("b.ts", source),
        ],
    );
    assert_eq!(project.codes("b.ts"), vec![Some(1192)]);
}

#[test]
fn a_type_only_import_of_a_missing_name_is_reported_too() {
    let project = run(
        "type-only",
        &[
            ("a.ts", "export interface User { name: string }"),
            ("b.ts", "import type { Usr } from './a';"),
        ],
    );
    assert_eq!(project.codes("b.ts"), vec![Some(2305)]);
}

#[test]
fn correct_imports_stay_clean() {
    let project = run(
        "clean",
        &[
            (
                "a.ts",
                "export interface User { name: string }\nexport default function f(): number { return 1 }",
            ),
            (
                "b.ts",
                "import f, { User } from './a'; const u: User = { name: 'x' }; const n: number = f();",
            ),
        ],
    );
    assert!(project.errors("b.ts").is_empty());
}

#[test]
fn a_missing_name_is_not_reported_against_a_module_that_left_exports_out() {
    // `class K` cannot be exported yet, so a.ts's list is partial: `K` might be there.
    let project = run(
        "partial",
        &[
            (
                "a.ts",
                "export interface User { name: string }\nexport class K {}",
            ),
            ("b.ts", "import { K } from './a';"),
        ],
    );
    assert!(project.errors("b.ts").is_empty());
}

#[test]
fn a_re_export_of_a_missing_name_or_file_is_reported() {
    let project = run(
        "re-export",
        &[
            ("a.ts", "export interface User { name: string }"),
            (
                "b.ts",
                "export { Usr } from './a';\nexport { Z } from './gone';",
            ),
        ],
    );
    assert_eq!(project.codes("b.ts"), vec![Some(2305), Some(2307)]);
}

#[test]
fn a_side_effect_import_of_a_missing_file_is_ts2307() {
    let project = run("side-effect", &[("b.ts", "import './gone';")]);
    assert_eq!(project.codes("b.ts"), vec![Some(2307)]);
}
