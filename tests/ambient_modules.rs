#![cfg(feature = "module-resolution")]

use std::fs;
use std::path::{Path, PathBuf};

use tsox::{Diagnostic, FileOutcome, ModuleGraph, ModuleResolver, Severity, check_project};

// `declare module "x" { ... }` in a script declaration file is a module that answers the
// specifier `x` for every file of the project, whether or not any file by that name exists.

struct Checked {
    graph: ModuleGraph,
    report: tsox::ProjectReport,
}

fn run(test: &str, files: &[(&str, &str)]) -> Checked {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("ambient-{test}"));
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

const LEGACY: &str = "declare module \"legacy\" {
    export function parse(s: string): number;
    export interface Opts { strict: boolean }
    export const VERSION: string;
}";

#[test]
fn an_ambient_module_types_the_imports_that_name_it() {
    let project = run(
        "basic",
        &[
            ("types.d.ts", LEGACY),
            (
                "b.ts",
                "import { parse, Opts, VERSION } from 'legacy';\nconst a: string = parse('1');\nconst o: Opts = { strict: 1 };\nconst v: number = VERSION;\nconst ok: number = parse('2');",
            ),
        ],
    );
    assert_eq!(
        project.codes("b.ts"),
        vec![Some(2322); 3],
        "{:?}",
        project.errors("b.ts")
    );
}

#[test]
fn a_name_the_ambient_module_does_not_export_is_ts2305() {
    let project = run(
        "missing-name",
        &[
            ("types.d.ts", LEGACY),
            ("b.ts", "import { nope } from 'legacy';"),
        ],
    );
    assert_eq!(project.codes("b.ts"), vec![Some(2305)]);
}

#[test]
fn every_member_is_exported_when_the_block_marks_none() {
    let project = run(
        "export-all",
        &[
            (
                "types.d.ts",
                "declare module \"bare\" {\n  function f(n: number): number;\n  interface P { x: number }\n}",
            ),
            (
                "b.ts",
                "import { f, P } from 'bare';\nconst s: string = f(1);\nconst p: P = { x: 'a' };",
            ),
        ],
    );
    assert_eq!(
        project.codes("b.ts"),
        vec![Some(2322); 2],
        "{:?}",
        project.errors("b.ts")
    );
}

#[test]
fn a_default_export_of_an_ambient_module_is_typed() {
    let project = run(
        "default",
        &[
            (
                "types.d.ts",
                "declare module \"widget\" {\n  export default function make(n: number): string;\n}",
            ),
            (
                "b.ts",
                "import make from 'widget';\nconst n: number = make(1);",
            ),
        ],
    );
    assert_eq!(
        project.codes("b.ts"),
        vec![Some(2322)],
        "{:?}",
        project.errors("b.ts")
    );
}

#[test]
fn a_namespace_import_of_an_ambient_module_has_its_values_and_types() {
    let project = run(
        "star-as",
        &[
            ("types.d.ts", LEGACY),
            (
                "b.ts",
                "import * as lib from 'legacy';\nconst a: string = lib.parse('1');\nconst o: lib.Opts = { strict: 1 };",
            ),
        ],
    );
    assert_eq!(
        project.codes("b.ts"),
        vec![Some(2322); 2],
        "{:?}",
        project.errors("b.ts")
    );
}

#[test]
fn two_blocks_for_one_specifier_are_their_union() {
    let project = run(
        "union",
        &[
            (
                "types.d.ts",
                "declare module \"m\" { export const a: number; }\ndeclare module \"m\" { export const b: string; }",
            ),
            (
                "b.ts",
                "import { a, b } from 'm';\nconst x: string = a;\nconst y: number = b;",
            ),
        ],
    );
    assert_eq!(
        project.codes("b.ts"),
        vec![Some(2322); 2],
        "{:?}",
        project.errors("b.ts")
    );
}

#[test]
fn an_ambient_module_wins_over_a_file_of_the_same_specifier() {
    let project = run(
        "precedence",
        &[
            (
                "types.d.ts",
                "declare module \"./local\" { export const k: string; }",
            ),
            ("local.ts", "export const k = 1;"),
            ("b.ts", "import { k } from './local';\nconst n: number = k;"),
        ],
    );
    assert_eq!(
        project.codes("b.ts"),
        vec![Some(2322)],
        "{:?}",
        project.errors("b.ts")
    );
}

#[test]
fn a_block_inside_a_module_file_is_an_augmentation_and_is_not_read_as_ambient() {
    let project = run(
        "augmentation",
        &[
            (
                "aug.d.ts",
                "import './other';\ndeclare module \"legacy\" { export const x: number; }\nexport {};",
            ),
            ("other.ts", "export const o = 1;"),
            ("b.ts", "import { x, anything } from 'legacy';"),
        ],
    );
    // The block augments a module that does not exist, so it declares nothing: `x` is not
    // typed and `anything` is not called a missing member. The import itself may be called
    // a missing module (TS2307), which is what TypeScript says, unless the surrounding
    // configuration could be hiding it.
    let codes: Vec<_> = project
        .errors("b.ts")
        .iter()
        .map(|d| d.code.tsc_code())
        .collect();
    assert!(
        codes.is_empty() || codes == vec![Some(2307)],
        "{:?}",
        project.errors("b.ts")
    );
}

#[test]
fn a_shorthand_ambient_module_declares_nothing_to_report_against() {
    let project = run(
        "shorthand",
        &[
            ("types.d.ts", "declare module \"untyped\";"),
            ("b.ts", "import { anything } from 'untyped';"),
        ],
    );
    assert!(
        project.errors("b.ts").is_empty(),
        "{:?}",
        project.errors("b.ts")
    );
}

#[test]
fn a_member_with_a_namespace_inside_an_ambient_module_is_typed() {
    let project = run(
        "namespace-inside",
        &[
            (
                "types.d.ts",
                "declare module \"lib\" {\n  export namespace Util {\n    function twice(n: number): number;\n    interface Cfg { on: boolean }\n  }\n}",
            ),
            (
                "b.ts",
                "import { Util } from 'lib';\nconst s: string = Util.twice(1);\nconst c: Util.Cfg = { on: 1 };",
            ),
        ],
    );
    assert_eq!(
        project.codes("b.ts"),
        vec![Some(2322); 2],
        "{:?}",
        project.errors("b.ts")
    );
}

#[test]
fn the_declaration_file_itself_is_still_in_the_report() {
    let project = run(
        "reported",
        &[
            ("types.d.ts", LEGACY),
            ("b.ts", "import { parse } from 'legacy';"),
        ],
    );
    assert!(project.errors("types.d.ts").is_empty());
}
