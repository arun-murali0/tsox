#![cfg(feature = "module-resolution")]

use std::fs;
use std::path::Path;

use tsox::{FileOutcome, ModuleGraph, ModuleResolver, ProjectConfig, Severity, check_project};

// `types` names the declaration packages a project takes without importing them, looked up in
// `typeRoots` first and then as ordinary packages. Their global declarations and ambient modules
// become part of the program. Unset, `types` takes nothing: TypeScript 6.0, the version this
// checker is measured against, changed the default to `[]`. Every expectation below was checked
// against `tsc` 6.0.3.

const USE: &str = "export const a: FooGlobal = { foo: 1 };\n\
export const c: string = fooValue.foo;\n\
import { barValue } from 'bar-ambient';\n\
export const d: string = barValue;\n\
export const e: LocalGlobal = { l: 1 };\n\
export const f: BazGlobal = { z: 1 };\n";

fn codes(test: &str, extra_options: &str, extra_files: &[(&str, &str)]) -> Vec<String> {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("typeroots-{test}"));
    let _ = fs::remove_dir_all(&root);
    let config = format!(
        "{{ \"compilerOptions\": {{ \"strict\": true, \"module\": \"esnext\", \
         \"moduleResolution\": \"bundler\"{extra_options} }}, \"include\": [\"use.ts\"] }}"
    );
    let mut files: Vec<(&str, &str)> = vec![
        ("tsconfig.json", config.as_str()),
        ("use.ts", USE),
        (
            "node_modules/@types/foo/index.d.ts",
            "interface FooGlobal { foo: number }\ndeclare const fooValue: FooGlobal;\n",
        ),
        (
            "node_modules/@types/foo/package.json",
            r#"{ "name": "@types/foo", "version": "1.0.0", "types": "index.d.ts" }"#,
        ),
        (
            "node_modules/@types/bar/index.d.ts",
            "declare module 'bar-ambient' { export const barValue: number; }\n",
        ),
        (
            "node_modules/@types/bar/package.json",
            r#"{ "name": "@types/bar", "version": "1.0.0", "types": "index.d.ts" }"#,
        ),
        (
            "node_modules/local-pkg/index.d.ts",
            "interface LocalGlobal { l: number }\n",
        ),
        (
            "node_modules/local-pkg/package.json",
            r#"{ "name": "local-pkg", "version": "1.0.0", "types": "index.d.ts" }"#,
        ),
        (
            "mytypes/baz/index.d.ts",
            "interface BazGlobal { z: number }\n",
        ),
    ];
    files.extend_from_slice(extra_files);
    for (name, text) in &files {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
    }
    let config = ProjectConfig::load(root.join("tsconfig.json")).expect("the config loads");
    let resolver =
        ModuleResolver::with_tsconfig(config.config_file()).with_node_next(config.uses_node_next());
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
        let name = graph.files().path(file.file_id).unwrap();
        if !name.ends_with("use.ts") {
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

const NOTHING_LOADED: [&str; 5] = [
    "Some(2304)",
    "Some(2304)",
    "Some(2304)",
    "Some(2304)",
    "Some(2307)",
];

#[test]
fn with_no_types_option_nothing_is_taken() {
    // Not FooGlobal, fooValue, LocalGlobal or BazGlobal, and not the ambient module either, even
    // though `node_modules/@types` has packages: TypeScript 6.0 takes none unless `types` lists them.
    assert_eq!(codes("default", "", &[]), NOTHING_LOADED);
}

#[test]
fn an_empty_types_list_takes_nothing() {
    assert_eq!(codes("empty", ", \"types\": []", &[]), NOTHING_LOADED);
}

#[test]
fn type_roots_alone_take_nothing() {
    assert_eq!(
        codes("roots-only", ", \"typeRoots\": [\"./mytypes\"]", &[]),
        NOTHING_LOADED
    );
}

#[test]
fn a_listed_package_is_read_and_the_others_stay_out() {
    // foo is taken: its global is typed, so `c` is a wrong use. bar is not, so its ambient module
    // is a missing module; LocalGlobal and BazGlobal are missing names.
    assert_eq!(
        codes("one-package", ", \"types\": [\"foo\"]", &[]),
        ["Some(2304)", "Some(2304)", "Some(2307)", "Some(2322)"]
    );
}

#[test]
fn types_find_a_package_in_the_type_roots_and_one_outside_them() {
    // foo and bar come from node_modules/@types, local-pkg from node_modules by an ordinary
    // lookup; bar's ambient module is found, so only BazGlobal is missing.
    assert_eq!(
        codes(
            "several",
            ", \"types\": [\"local-pkg\", \"foo\", \"bar\"]",
            &[]
        ),
        ["Some(2304)", "Some(2322)", "Some(2322)"]
    );
}

#[test]
fn custom_type_roots_replace_the_default_ones() {
    // `baz` comes from ./mytypes; @types is not searched, so foo's names are missing.
    assert_eq!(
        codes(
            "custom-roots",
            ", \"typeRoots\": [\"./mytypes\"], \"types\": [\"baz\"]",
            &[]
        ),
        ["Some(2304)", "Some(2304)", "Some(2304)", "Some(2307)"]
    );
}

#[test]
fn a_types_entry_may_name_a_package_subpath() {
    // The subpath file is read, so SubGlobal is known; the package root is not, so LocalGlobal is
    // missing. Same as `tsc`.
    let result = codes(
        "subpath",
        ", \"types\": [\"local-pkg/sub\"]",
        &[
            (
                "use.ts",
                "export const g: SubGlobal = { s: 1 };\nexport const e: LocalGlobal = { l: 1 };\n",
            ),
            (
                "node_modules/local-pkg/sub.d.ts",
                "interface SubGlobal { s: number }\n",
            ),
            (
                "node_modules/local-pkg/package.json",
                r#"{ "name": "local-pkg", "version": "1.0.0", "types": "index.d.ts",
                   "exports": { ".": { "types": "./index.d.ts" },
                                "./sub": { "types": "./sub.d.ts" } } }"#,
            ),
        ],
    );
    assert_eq!(result, ["Some(2304)"]);
}
