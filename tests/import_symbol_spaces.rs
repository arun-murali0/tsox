#![cfg(feature = "module-resolution")]

use std::fs;
use std::path::{Path, PathBuf};

use tsox::{FileOutcome, ModuleGraph, ModuleResolver, ProjectConfig, Severity, check_project};

fn run(test: &str, files: &[(&str, &str)]) -> Vec<(String, Vec<Option<u32>>)> {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("symbol-spaces-{test}"));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();

    fs::write(
        root.join("tsconfig.json"),
        r#"{"compilerOptions":{"strict":true,"module":"esnext","moduleResolution":"bundler"},"include":["*.ts"]}"#,
    )
    .unwrap();

    let mut entries: Vec<PathBuf> = Vec::new();
    for (name, text) in files {
        let path = root.join(name);
        fs::write(&path, text).unwrap();
        entries.push(path);
    }

    let config = ProjectConfig::load(root.join("tsconfig.json")).expect("the config loads");
    let resolver = ModuleResolver::with_tsconfig(config.config_file());
    let graph = ModuleGraph::build(&entries, &resolver)
        .expect("the project graph builds")
        .with_root(&root)
        .with_config(&config);
    let report = check_project(&graph);

    let mut result = Vec::new();
    for file in &report.files {
        let Some(path) = graph.files().path(file.file_id) else {
            continue;
        };
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let FileOutcome::Checked(diagnostics) = &file.outcome else {
            continue;
        };
        let mut codes: Vec<_> = diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == Severity::Error)
            .map(|diagnostic| diagnostic.code.tsc_code())
            .collect();
        codes.sort();
        result.push((name, codes));
    }
    result.sort_by(|left, right| left.0.cmp(&right.0));
    result
}

fn codes(result: &[(String, Vec<Option<u32>>)], file: &str) -> Vec<Option<u32>> {
    result
        .iter()
        .find(|(name, _)| name == file)
        .unwrap_or_else(|| panic!("{file} is not in the report"))
        .1
        .clone()
}

const DEFINITIONS: &str = r#"
export class Token { value: number = 1; }
export interface Shape { x: string; }
export enum Mode { Ready }
export function makeToken(): Token { return new Token(); }
export const version = 1;
"#;

#[test]
fn type_only_class_import_has_type_side_but_not_value_side() {
    let result = run(
        "type-only-class",
        &[
            ("defs.ts", DEFINITIONS),
            (
                "use.ts",
                "import type { Token } from './defs';\n\
                 let token: Token = { value: 1 };\n\
                 Token;\n",
            ),
        ],
    );
    assert_eq!(codes(&result, "use.ts"), [Some(1361)]);
}

#[test]
fn inline_type_only_class_import_has_no_runtime_binding() {
    let result = run(
        "inline-type-only-class",
        &[
            ("defs.ts", DEFINITIONS),
            (
                "use.ts",
                "import { type Token } from './defs';\n\
                 let token: Token = { value: 1 };\n\
                 Token;\n",
            ),
        ],
    );
    assert_eq!(codes(&result, "use.ts"), [Some(1361)]);
}

#[test]
fn type_only_value_import_reports_ts1361() {
    let result = run(
        "type-only-function",
        &[
            ("defs.ts", DEFINITIONS),
            (
                "use.ts",
                "import type { makeToken, version, Mode } from './defs';\n\
                 makeToken();\n\
                 version;\n\
                 Mode.Ready;\n",
            ),
        ],
    );
    assert_eq!(
        codes(&result, "use.ts"),
        [Some(1361), Some(1361), Some(1361)]
    );
}

#[test]
fn importing_a_type_only_interface_as_a_value_reports_ts2693_not_ts1361() {
    let result = run(
        "type-only-interface",
        &[
            ("defs.ts", DEFINITIONS),
            (
                "use.ts",
                "import type { Shape } from './defs';\n\
                 Shape;\n",
            ),
        ],
    );
    assert_eq!(codes(&result, "use.ts"), [Some(2693)]);
}

#[test]
fn ordinary_import_of_an_interface_is_type_only_for_value_resolution() {
    let result = run(
        "ordinary-interface",
        &[
            ("defs.ts", DEFINITIONS),
            (
                "use.ts",
                "import { Shape } from './defs';\n\
                 const shape: Shape = { x: 'ok' };\n\
                 Shape;\n",
            ),
        ],
    );
    assert_eq!(codes(&result, "use.ts"), [Some(2693)]);
}

#[test]
fn local_interfaces_and_aliases_are_type_space_only() {
    let result = run(
        "local-type-only",
        &[(
            "use.ts",
            "interface LocalShape { x: number; }\n\
             type LocalAlias = string;\n\
             LocalShape;\n\
             LocalAlias;\n",
        )],
    );
    assert_eq!(codes(&result, "use.ts"), [Some(2693), Some(2693)]);
}

#[test]
fn ordinary_class_import_has_both_type_and_value_sides() {
    let result = run(
        "ordinary-class",
        &[
            ("defs.ts", DEFINITIONS),
            (
                "use.ts",
                "import { Token } from './defs';\n\
                 const token: Token = new Token();\n\
                 token.value;\n",
            ),
        ],
    );
    assert!(codes(&result, "use.ts").is_empty());
}

#[test]
fn type_only_namespace_import_exposes_qualified_types_but_not_a_value() {
    let result = run(
        "type-only-namespace",
        &[
            (
                "defs.ts",
                "export namespace Api { export interface Request { id: number; } export const version = 1; }\n",
            ),
            (
                "use.ts",
                "import type * as Types from './defs';\n\
                 const request: Types.Api.Request = { id: 1 };\n\
                 Types;\n",
            ),
        ],
    );
    assert_eq!(codes(&result, "use.ts"), [Some(1361)]);
}
