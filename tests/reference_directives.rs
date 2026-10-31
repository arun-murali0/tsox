#![cfg(feature = "module-resolution")]

use std::fs;
use std::path::Path;

use tsox::{FileOutcome, ModuleGraph, ModuleResolver, ProjectConfig, Severity, check_project};

// `/// <reference path="..." />` and `/// <reference types="..." />` at the top of a file bring
// another file into the program, so the globals it declares are visible. Every expectation was
// checked against `tsc` 6.0.3.

fn codes(test: &str, config_extra: &str, include: &str, files: &[(&str, &str)]) -> Vec<String> {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("refdirectives-{test}"));
    let _ = fs::remove_dir_all(&root);
    let config = format!(
        "{{ \"compilerOptions\": {{ \"strict\": true, \"module\": \"esnext\", \
         \"moduleResolution\": \"bundler\"{config_extra} }}, \"include\": [{include}] }}"
    );
    let mut all: Vec<(&str, &str)> = vec![("tsconfig.json", config.as_str())];
    all.extend_from_slice(files);
    for (name, text) in &all {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
    }
    let config = ProjectConfig::load(root.join("tsconfig.json")).expect("the config loads");
    let resolver = ModuleResolver::with_tsconfig(config.config_file());
    let entries: Vec<_> = config
        .project_entries(&resolver)
        .into_iter()
        .map(|path| fs::canonicalize(path).unwrap())
        .collect();
    let graph = ModuleGraph::build(&entries, &resolver)
        .expect("the project builds")
        .with_root(&root)
        .with_config(&config)
        .with_type_entries_loaded();
    let report = check_project(&graph);
    let mut out = Vec::new();
    for file in &report.files {
        if !graph
            .files()
            .path(file.file_id)
            .unwrap()
            .ends_with("use.ts")
        {
            continue;
        }
        let FileOutcome::Checked(diagnostics) = &file.outcome else {
            panic!("use.ts was not checked");
        };
        out.extend(
            diagnostics
                .iter()
                .filter(|d| d.severity == Severity::Error)
                .map(|d| format!("{:?}", d.code.tsc_code())),
        );
    }
    out.sort();
    out
}

const TYPES_PACKAGE: &[(&str, &str)] = &[
    (
        "node_modules/@types/pkg/index.d.ts",
        "/// <reference path=\"extra.d.ts\" />\n/// <reference types=\"other\" />\n\
         interface PkgGlobal { p: number }\n",
    ),
    (
        "node_modules/@types/pkg/extra.d.ts",
        "interface ExtraGlobal { e: number }\n",
    ),
    (
        "node_modules/@types/pkg/package.json",
        r#"{ "name": "@types/pkg", "version": "1.0.0", "types": "index.d.ts" }"#,
    ),
    (
        "node_modules/@types/other/index.d.ts",
        "interface OtherGlobal { o: number }\n",
    ),
    (
        "node_modules/@types/other/package.json",
        r#"{ "name": "@types/other", "version": "1.0.0", "types": "index.d.ts" }"#,
    ),
];

#[test]
fn a_types_package_brings_in_the_files_it_references() {
    // `types: ["pkg"]` reads index.d.ts, which references extra.d.ts by path and the `other`
    // package by name; all three declare a global the file uses. Only the wrong use is reported.
    let mut files = TYPES_PACKAGE.to_vec();
    files.push((
        "use.ts",
        "export const a: PkgGlobal = { p: 1 };\n\
         export const b: ExtraGlobal = { e: 1 };\n\
         export const c: OtherGlobal = { o: 1 };\n\
         export const bad: ExtraGlobal = { e: 's' };\n",
    ));
    let result = codes("package", ", \"types\": [\"pkg\"]", "\"use.ts\"", &files);
    assert_eq!(result, ["Some(2322)"]);
}

#[test]
fn a_directive_after_the_first_statement_brings_in_nothing() {
    let result = codes(
        "late",
        "",
        "\"use.ts\", \"main.d.ts\"",
        &[
            (
                "main.d.ts",
                "interface Early { a: number }\n/// <reference path=\"late.d.ts\" />\n",
            ),
            ("late.d.ts", "interface LateGlobal { l: number }\n"),
            (
                "use.ts",
                "export const x: Early = { a: 1 };\nexport const y: LateGlobal = { l: 1 };\n",
            ),
        ],
    );
    assert_eq!(result, ["Some(2304)"]);
}

#[test]
fn a_directive_in_a_project_file_brings_in_the_file_it_names() {
    // The file the project selects references a declaration file nothing else names or imports.
    let result = codes(
        "project-file",
        "",
        "\"use.ts\"",
        &[
            ("globals.d.ts", "interface FromReference { n: number }\n"),
            (
                "use.ts",
                "/// <reference path=\"globals.d.ts\" />\n\
                 export const ok: FromReference = { n: 1 };\n\
                 export const bad: FromReference = { n: 's' };\n",
            ),
        ],
    );
    assert_eq!(result, ["Some(2322)"]);
}

#[test]
fn a_directive_that_names_nothing_is_reported_as_tsc_does() {
    // tsc: TS2688 for a `types` directive, TS6053 for a `path` directive.
    let result = codes(
        "missing",
        "",
        "\"use.ts\"",
        &[(
            "use.ts",
            "/// <reference types=\"nothing-here\" />\n\
             /// <reference path=\"nothing.d.ts\" />\n\
             export const x = 1;\n",
        )],
    );
    assert_eq!(result, ["Some(2688)", "Some(6053)"]);
}

#[test]
fn lib_and_no_default_lib_directives_are_accepted_and_ignored() {
    let result = codes(
        "lib",
        "",
        "\"use.ts\"",
        &[(
            "use.ts",
            "/// <reference lib=\"es2020\" />\n\
             /// <reference no-default-lib=\"true\" />\n\
             export const x: number = 1;\n",
        )],
    );
    assert!(result.is_empty(), "{result:?}");
}

#[test]
fn a_directive_that_resolves_is_not_reported() {
    let result = codes(
        "resolves",
        "",
        "\"use.ts\"",
        &[
            ("globals.d.ts", "interface G { n: number }\n"),
            (
                "use.ts",
                "/// <reference path=\"globals.d.ts\" />\nexport const g: G = { n: 1 };\n",
            ),
        ],
    );
    assert!(result.is_empty(), "{result:?}");
}
