#![cfg(feature = "module-resolution")]

use std::fs;
use std::path::Path;

use tsox::{FileOutcome, ModuleGraph, ModuleResolver, ProjectConfig, Severity, check_project};

fn error_codes(test: &str, compiler_options: &str, source: &str) -> Vec<Option<u32>> {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("stdlib-global-values-{test}"));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("tsconfig.json"),
        format!("{{\"compilerOptions\":{{{compiler_options}}},\"files\":[\"main.ts\"]}}"),
    )
    .unwrap();
    fs::write(root.join("main.ts"), source).unwrap();

    let config = ProjectConfig::load(root.join("tsconfig.json")).expect("the config loads");
    let resolver = ModuleResolver::with_tsconfig(config.config_file());
    let entries: Vec<_> = config
        .project_entries(&resolver)
        .into_iter()
        .map(|path| fs::canonicalize(path).expect("project entry exists"))
        .collect();
    let graph = ModuleGraph::build(&entries, &resolver)
        .expect("the project graph builds")
        .with_root(&root)
        .with_config(&config);
    let report = check_project(&graph);
    let file = report
        .files
        .iter()
        .find(|file| {
            graph
                .files()
                .path(file.file_id)
                .is_some_and(|path| path == root.join("main.ts"))
        })
        .expect("main.ts is reported");
    let FileOutcome::Checked(diagnostics) = &file.outcome else {
        panic!("main.ts was not checked: {:?}", file.outcome);
    };
    let mut codes: Vec<_> = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .map(|diagnostic| diagnostic.code.tsc_code())
        .collect();
    codes.sort();
    codes
}

#[test]
fn es5_global_values_are_imported_from_the_selected_library() {
    let codes = error_codes(
        "es5-values",
        r#""lib":["ES5"]"#,
        r#"
const max: number = Math.max(1, 2);
const date: Date = new Date();
const timestamp: number = date.getTime();
const array: Array<number> = new Array<number>(1, 2);
const error: Error = new Error("failed");
const json = JSON.stringify({ max });
"#,
    );
    assert!(codes.is_empty(), "expected no errors, got {codes:?}");
}

#[test]
fn selected_es2015_library_registers_promise_and_map_values() {
    let codes = error_codes(
        "es2015-values",
        r#""lib":["ES2015"]"#,
        r#"
const promise: Promise<number> = Promise.resolve(42);
const map: Map<string, number> = new Map<string, number>();
map.set("answer", 42);
"#,
    );
    assert!(codes.is_empty(), "expected no errors, got {codes:?}");
}

#[test]
fn loaded_global_values_are_not_silently_treated_as_any() {
    let codes = error_codes("typed-math", r#""lib":["ES5"]"#, "Math.noSuchMethod();\n");
    assert!(
        codes.contains(&Some(2339)),
        "expected TS2339 for an absent Math member, got {codes:?}"
    );
}

#[test]
fn imported_global_function_values_keep_their_parameter_types() {
    let codes = error_codes(
        "typed-global-function",
        r#""lib":["ES5"]"#,
        r#"const value: number = parseInt("42");
parseInt(true);
"#,
    );
    assert!(
        codes.contains(&Some(2345)),
        "expected TS2345 for an invalid parseInt argument, got {codes:?}"
    );
}

#[test]
fn type_only_standard_library_names_do_not_become_runtime_values() {
    let codes = error_codes("type-only-iterable", r#""lib":["ES2015"]"#, "Iterable;\n");
    assert!(
        codes.contains(&Some(2693)),
        "expected TS2693 for using type-only Iterable as a value, got {codes:?}"
    );
}

#[test]
fn dom_global_values_exist_only_when_dom_is_selected() {
    let selected = error_codes(
        "dom-selected",
        r#""lib":["ES5","DOM"]"#,
        "document.title;\n",
    );
    assert!(
        selected.is_empty(),
        "expected DOM global to resolve, got {selected:?}"
    );

    let unselected = error_codes("dom-unselected", r#""lib":["ES5"]"#, "document.title;\n");
    assert!(
        unselected.contains(&Some(2304)),
        "expected TS2304 when DOM is not selected, got {unselected:?}"
    );
}

#[test]
fn no_lib_does_not_use_the_legacy_ambient_value_fallback() {
    let codes = error_codes("no-lib-values", r#""noLib":true"#, "Math.max(1, 2);\n");
    assert!(
        codes.contains(&Some(2304)),
        "expected TS2304 with noLib, got {codes:?}"
    );
}
