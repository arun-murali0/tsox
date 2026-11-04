#![cfg(feature = "module-resolution")]

use std::fs;
use std::path::Path;

use tsox::{FileOutcome, ModuleGraph, ModuleResolver, ProjectConfig, Severity, check_project};

fn error_codes(test: &str, compiler_options: &str, source: &str) -> Vec<Option<u32>> {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("stdlib-global-types-{test}"));
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
fn selected_library_generic_types_are_registered_with_their_arity() {
    let codes = error_codes(
        "selected-generic",
        r#""lib":["ES5"]"#,
        "type Good = Array<number>;\ntype MissingArgument = Array;\n",
    );
    assert_eq!(codes, [Some(2314)]);
}

#[test]
fn a_global_type_not_in_the_selected_library_set_is_missing() {
    let codes = error_codes(
        "unselected-promise",
        r#""lib":["ES5"]"#,
        "type Missing = Iterable<number>;\n",
    );
    assert_eq!(codes, [Some(2304)]);
}

#[test]
fn no_lib_does_not_accept_names_from_the_generated_global_name_registry() {
    let codes = error_codes(
        "no-lib",
        r#""noLib":true"#,
        "type Missing = Array<number>;\n",
    );
    assert!(
        codes.contains(&Some(2304)),
        "expected TS2304, got {codes:?}"
    );
}

#[test]
fn selected_es2015_library_registers_map_as_a_generic_type() {
    let codes = error_codes(
        "map-generic",
        r#""lib":["ES2015"]"#,
        "type Good = Map<string, number>;\ntype MissingArgument = Map;\n",
    );
    assert_eq!(codes, [Some(2314)]);
}

#[test]
fn explicit_empty_lib_does_not_inherit_generated_global_type_names() {
    let codes = error_codes(
        "empty-lib",
        r#""lib":[]"#,
        "type Missing = Array<number>;\n",
    );
    assert!(
        codes.contains(&Some(2304)),
        "expected TS2304, got {codes:?}"
    );
}
