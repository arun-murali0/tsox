#![cfg(feature = "module-resolution")]

use std::fs;
use std::path::{Path, PathBuf};

use tsox::{Diagnostic, FileOutcome, ModuleGraph, ModuleResolver, Severity, check_project};

// Step 8: a package's declaration file is read for what it exports. It gets a Package key
// (name, version, path inside the package), its exports cross into the project, and it is
// never reported on.

struct Checked {
    root: PathBuf,
    graph: ModuleGraph,
    report: tsox::ProjectReport,
}

fn run(test: &str, files: &[(&str, &str)], entries: &[&str]) -> Checked {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("package-imports-{test}"));
    let _ = fs::remove_dir_all(&root);
    for (name, source) in files {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, source).unwrap();
    }
    let paths: Vec<PathBuf> = entries.iter().map(|e| root.join(e)).collect();
    let graph = ModuleGraph::build(&paths, &ModuleResolver::new())
        .expect("the project builds")
        .with_root(root.clone());
    let report = check_project(&graph);
    Checked {
        root,
        graph,
        report,
    }
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

    fn reported(&self, file: &str) -> bool {
        self.report.files.iter().any(|report| {
            self.graph
                .files()
                .path(report.file_id)
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy()
                == file
        })
    }
}

const MANIFEST: &str = r#"{ "name": "left-pad", "version": "1.3.0", "dependencies": { "x": { "version": "9" } }, "types": "index.d.ts" }"#;
const DTS: &str = "export declare function pad(s: string, n: number): string;\nexport interface Options { width: number }\nexport declare const VERSION: string;";

#[test]
fn a_package_function_is_typed_from_its_declaration_file() {
    let project = run(
        "function",
        &[
            ("node_modules/left-pad/package.json", MANIFEST),
            ("node_modules/left-pad/index.d.ts", DTS),
            (
                "b.ts",
                "import { pad, Options, VERSION } from 'left-pad';\nconst a: number = pad('x', 2);\nconst o: Options = { width: 'x' };\nconst v: number = VERSION;\npad(1, 2);",
            ),
        ],
        &["b.ts"],
    );
    let codes: Vec<_> = project
        .errors("b.ts")
        .iter()
        .map(|d| d.code.tsc_code())
        .collect();
    assert_eq!(
        codes,
        vec![Some(2322), Some(2322), Some(2322), Some(2345)],
        "{:?}",
        project.errors("b.ts")
    );
}

#[test]
fn a_packages_declaration_file_is_never_reported_on() {
    let project = run(
        "not-reported",
        &[
            ("node_modules/left-pad/package.json", MANIFEST),
            (
                "node_modules/left-pad/index.d.ts",
                "export declare const x: string = 1;",
            ),
            ("b.ts", "import { x } from 'left-pad';"),
        ],
        &["b.ts"],
    );
    assert!(project.reported("b.ts"));
    assert!(!project.reported("index.d.ts"));
}

#[test]
fn a_package_file_gets_a_package_key_from_its_manifest() {
    let project = run(
        "key",
        &[
            ("node_modules/left-pad/package.json", MANIFEST),
            ("node_modules/left-pad/index.d.ts", DTS),
            ("b.ts", "import { pad } from 'left-pad';"),
        ],
        &["b.ts"],
    );
    let id = project
        .graph
        .files()
        .id(&project.root.join("node_modules/left-pad/index.d.ts"));
    let key = project
        .graph
        .module_key(id.expect("the package file is in the graph"));
    assert_eq!(
        key,
        Some(tsox::ModuleKey::package("left-pad", "1.3.0", "index.d.ts"))
    );
}

#[test]
fn a_manifest_without_a_version_gives_no_key_and_binds_nothing() {
    let project = run(
        "no-version",
        &[
            (
                "node_modules/left-pad/package.json",
                r#"{ "name": "left-pad" }"#,
            ),
            ("node_modules/left-pad/index.d.ts", DTS),
            (
                "b.ts",
                "import { pad } from 'left-pad';\nconst a: number = pad('x', 2);",
            ),
        ],
        &["b.ts"],
    );
    assert!(project.errors("b.ts").is_empty());
}

#[test]
fn a_scoped_package_is_keyed_by_its_scope_and_name() {
    let project = run(
        "scoped",
        &[
            (
                "node_modules/@acme/util/package.json",
                r#"{ "name": "@acme/util", "version": "2.0.0", "types": "lib/index.d.ts" }"#,
            ),
            (
                "node_modules/@acme/util/lib/index.d.ts",
                "export declare function twice(n: number): number;",
            ),
            (
                "b.ts",
                "import { twice } from '@acme/util';\nconst s: string = twice(1);",
            ),
        ],
        &["b.ts"],
    );
    assert_eq!(
        project.errors("b.ts").len(),
        1,
        "{:?}",
        project.errors("b.ts")
    );
}

#[test]
fn a_types_package_supplies_the_declarations() {
    let project = run(
        "at-types",
        &[
            (
                "node_modules/@types/lodashish/package.json",
                r#"{ "name": "@types/lodashish", "version": "4.0.0", "types": "index.d.ts" }"#,
            ),
            (
                "node_modules/@types/lodashish/index.d.ts",
                "export declare function chunk(n: number): number[];",
            ),
            (
                "node_modules/lodashish/package.json",
                r#"{ "name": "lodashish", "version": "4.0.0", "main": "index.js" }"#,
            ),
            ("node_modules/lodashish/index.js", "module.exports = {};"),
            (
                "b.ts",
                "import { chunk } from 'lodashish';\nconst s: string = chunk(1);",
            ),
        ],
        &["b.ts"],
    );
    assert_eq!(
        project.errors("b.ts").len(),
        1,
        "{:?}",
        project.errors("b.ts")
    );
}

#[test]
fn a_missing_name_is_not_reported_against_a_declaration_file_with_unexported_declarations() {
    // In a declaration file an unexported declaration may be exported anyway, so the list
    // is partial and `Hidden` must not be called missing.
    let project = run(
        "dts-partial",
        &[
            ("node_modules/left-pad/package.json", MANIFEST),
            (
                "node_modules/left-pad/index.d.ts",
                "declare interface Hidden { x: number }\nexport declare function pad(s: string): string;",
            ),
            ("b.ts", "import { Hidden } from 'left-pad';"),
        ],
        &["b.ts"],
    );
    assert!(
        project.errors("b.ts").is_empty(),
        "{:?}",
        project.errors("b.ts")
    );
}

#[test]
fn a_package_that_uses_export_equals_binds_nothing_and_reports_nothing() {
    let project = run(
        "export-equals",
        &[
            ("node_modules/left-pad/package.json", MANIFEST),
            (
                "node_modules/left-pad/index.d.ts",
                "declare function pad(s: string): string;\nexport = pad;",
            ),
            ("b.ts", "import { anything } from 'left-pad';"),
        ],
        &["b.ts"],
    );
    assert!(
        project.errors("b.ts").is_empty(),
        "{:?}",
        project.errors("b.ts")
    );
}

#[test]
fn a_declare_namespace_in_a_package_types_its_members() {
    let project = run(
        "dts-namespace",
        &[
            ("node_modules/left-pad/package.json", MANIFEST),
            (
                "node_modules/left-pad/index.d.ts",
                "export declare namespace Pad {\n  function left(s: string, n: number): string;\n  interface Options { width: number }\n  const VERSION: string;\n}",
            ),
            (
                "b.ts",
                "import { Pad } from 'left-pad';\nconst a: number = Pad.left('x', 1);\nconst o: Pad.Options = { width: 'x' };\nconst ok: string = Pad.VERSION;",
            ),
        ],
        &["b.ts"],
    );
    let codes: Vec<_> = project
        .errors("b.ts")
        .iter()
        .map(|d| d.code.tsc_code())
        .collect();
    assert_eq!(
        codes,
        vec![Some(2322), Some(2322)],
        "{:?}",
        project.errors("b.ts")
    );
}
