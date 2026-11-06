//! A project that cannot observe a DOM global gets the same diagnostics without the DOM
//! library files, and one that can still loads them.
#![cfg(feature = "module-resolution")]

use std::fs;
use std::path::Path;

use tsox::{
    FileOutcome, ModuleGraph, ModuleResolver, ProjectConfig, Severity, check_project,
    project_needs_dom_libraries,
};

/// Error codes of `main.ts` and the graph size, built either with every library file up front
/// or the way the command line does it: without the DOM files first, and again with them only
/// when the project needs them.
fn run(test: &str, source: &str, lazy: bool) -> (Vec<Option<u32>>, usize) {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("lazy-dom-{test}-{lazy}"));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("tsconfig.json"),
        r#"{"compilerOptions":{"strict":true,"target":"es2020"},"files":["main.ts"]}"#,
    )
    .unwrap();
    fs::write(root.join("main.ts"), source).unwrap();

    let config = ProjectConfig::load(root.join("tsconfig.json")).unwrap();
    let resolver = ModuleResolver::with_tsconfig(config.config_file());
    let canonical = |entries: Vec<std::path::PathBuf>| -> Vec<_> {
        entries
            .into_iter()
            .map(|path| fs::canonicalize(path).unwrap())
            .collect()
    };
    let eager = canonical(
        config
            .project_entries_with_standard_libraries(&resolver)
            .unwrap(),
    );
    let graph = if lazy {
        let lean = ModuleGraph::build(
            &canonical(config.project_entries_deferring_dom(&resolver).unwrap()),
            &resolver,
        )
        .unwrap();
        if project_needs_dom_libraries(&lean) {
            ModuleGraph::build(&eager, &resolver).unwrap()
        } else {
            lean
        }
    } else {
        ModuleGraph::build(&eager, &resolver).unwrap()
    };
    let graph = graph.with_root(&root).with_config(&config);
    let size = graph.len();
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
    (codes, size)
}

#[test]
fn a_project_without_dom_use_skips_the_dom_files_and_reports_the_same() {
    let source = "const a: number[] = [1];\nconst s: string = a;\nconst p: Promise<number> = Promise.resolve(1);\n";
    let (lazy, lazy_files) = run("plain", source, true);
    let (eager, eager_files) = run("plain", source, false);
    assert_eq!(lazy, eager);
    assert!(
        !lazy.is_empty(),
        "the fixture must produce an error to compare"
    );
    assert!(lazy_files < eager_files, "{lazy_files} vs {eager_files}");
}

#[test]
fn a_project_that_uses_a_dom_global_loads_the_dom_files() {
    let source = "const e = document.body;\nconst n: number = 'x';\n";
    let (lazy, lazy_files) = run("uses-dom", source, true);
    let (eager, eager_files) = run("uses-dom", source, false);
    assert_eq!(lazy, eager);
    assert_eq!(lazy_files, eager_files);
}

#[test]
fn a_script_that_merges_with_a_dom_interface_loads_the_dom_files() {
    let source = "interface Event { extra: number }\nconst n: number = 'x';\n";
    let (lazy, lazy_files) = run("merges-dom", source, true);
    let (eager, eager_files) = run("merges-dom", source, false);
    assert_eq!(lazy, eager);
    assert_eq!(lazy_files, eager_files);
}
