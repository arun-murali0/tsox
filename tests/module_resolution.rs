#![cfg(feature = "module-resolution")]

use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use tsox::{
    FileId, FileOutcome, ModuleEdge, ModuleError, ModuleGraph, ModuleResolver, check_project,
};

// Module resolution and the module graph. The fixtures are small projects on disk under
// tests/module-resolution-fixtures/, not under tests/fixtures/: they are directory
// trees whose imports are meant to resolve, and the tsc comparison harness checks every
// .ts file under tests/fixtures/ on its own, where those imports would all be errors.

fn fixture(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/module-resolution-fixtures")
        .join(relative)
}

fn build(entries: &[&str]) -> ModuleGraph {
    let entries: Vec<PathBuf> = entries.iter().map(|entry| fixture(entry)).collect();
    ModuleGraph::build(&entries, &ModuleResolver::new()).expect("fixture graph should build")
}

fn id_of(graph: &ModuleGraph, relative: &str) -> FileId {
    graph
        .files()
        .id(&fixture(relative))
        .unwrap_or_else(|| panic!("{relative} is not in the graph"))
}

fn edge<'a>(graph: &'a ModuleGraph, from: &str, specifier: &str) -> &'a ModuleEdge {
    graph
        .edges(id_of(graph, from))
        .iter()
        .find(|edge| edge.specifier == specifier)
        .unwrap_or_else(|| panic!("{from} has no edge for `{specifier}`"))
}

fn target_path(graph: &ModuleGraph, from: &str, specifier: &str) -> PathBuf {
    let target = edge(graph, from, specifier)
        .target
        .unwrap_or_else(|| panic!("`{specifier}` in {from} did not resolve"));
    graph
        .files()
        .path(target)
        .expect("a resolved target has a path")
        .to_path_buf()
}

fn names(graph: &ModuleGraph, ids: &[FileId]) -> Vec<String> {
    ids.iter()
        .map(|id| {
            graph
                .files()
                .path(*id)
                .and_then(|path| path.file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .expect("every id has a file name")
        })
        .collect()
}

fn all_ids(graph: &ModuleGraph) -> Vec<FileId> {
    (0..graph.len())
        .map(|position| FileId::new(u32::try_from(position).expect("fixture is small")))
        .collect()
}

#[test]
fn an_extensionless_relative_specifier_resolves_to_the_source_file() {
    let graph = build(&["basic/main.ts"]);
    assert_eq!(
        target_path(&graph, "basic/main.ts", "./math"),
        fixture("basic/math.ts")
    );
}

#[test]
fn a_directory_specifier_resolves_to_its_index_file() {
    let graph = build(&["basic/main.ts"]);
    assert_eq!(
        target_path(&graph, "basic/main.ts", "./shapes"),
        fixture("basic/shapes/index.ts")
    );
}

#[test]
fn a_js_specifier_resolves_to_the_typescript_file_next_to_it() {
    let graph = build(&["basic/main.ts"]);
    assert_eq!(
        target_path(&graph, "basic/main.ts", "./legacy.js"),
        fixture("basic/legacy.ts")
    );
}

#[test]
fn a_type_only_import_is_marked_and_resolved() {
    let graph = build(&["basic/main.ts"]);
    let import = edge(&graph, "basic/main.ts", "./point");
    assert!(import.is_type);
    assert!(import.is_import);
    assert_eq!(import.kind, tsox::ModuleRequestKind::TypeImport);
    assert_eq!(
        target_path(&graph, "basic/main.ts", "./point"),
        fixture("basic/point.ts")
    );
    assert!(!edge(&graph, "basic/main.ts", "./math").is_type);
}

#[test]
fn a_re_export_is_an_edge_that_binds_nothing() {
    let graph = build(&["basic/main.ts"]);
    let re_export = edge(&graph, "basic/main.ts", "./reexported");
    assert!(!re_export.is_import);
    assert_eq!(
        target_path(&graph, "basic/main.ts", "./reexported"),
        fixture("basic/reexported.ts")
    );
}

#[test]
fn an_unresolved_import_stays_as_an_edge_without_a_target() {
    let graph = build(&["basic/main.ts"]);
    assert_eq!(edge(&graph, "basic/main.ts", "./missing").target, None);

    let unresolved: Vec<_> = graph
        .unresolved()
        .map(|(from, edge)| (from, edge.specifier.as_str()))
        .collect();
    assert_eq!(
        unresolved,
        vec![(id_of(&graph, "basic/main.ts"), "./missing")]
    );
    // The file is not mistaken for one with fewer imports: all six statements are there.
    assert_eq!(graph.edges(id_of(&graph, "basic/main.ts")).len(), 6);
}

#[test]
fn package_exports_prefer_the_types_condition() {
    let graph = build(&["package-exports/app.ts"]);
    assert_eq!(
        target_path(&graph, "package-exports/app.ts", "widget"),
        fixture("package-exports/node_modules/widget/types/index.d.ts")
    );
}

#[test]
fn package_exports_subpaths_resolve() {
    let graph = build(&["package-exports/app.ts"]);
    assert_eq!(
        target_path(&graph, "package-exports/app.ts", "widget/gadget"),
        fixture("package-exports/node_modules/widget/types/gadget.d.ts")
    );
}

#[test]
fn a_hash_specifier_uses_the_nearest_package_imports_field() {
    let graph = build(&["package-imports/src/main.ts"]);
    assert_eq!(
        target_path(&graph, "package-imports/src/main.ts", "#internal/helper"),
        fixture("package-imports/src/internal/helper.ts")
    );
}

#[test]
fn a_nested_node_modules_package_shadows_the_hoisted_one() {
    let graph = build(&["nested-node-modules/app.ts"]);

    let from_app = target_path(&graph, "nested-node-modules/app.ts", "b");
    assert_eq!(
        from_app,
        fixture("nested-node-modules/node_modules/b/index.d.ts")
    );

    let from_a = target_path(&graph, "nested-node-modules/node_modules/a/index.d.ts", "b");
    assert_eq!(
        from_a,
        fixture("nested-node-modules/node_modules/a/node_modules/b/index.d.ts")
    );
}

#[test]
fn dependents_mirror_dependencies() {
    let graph = build(&["cycles/entry.ts"]);
    let entry = id_of(&graph, "cycles/entry.ts");
    let ring_a = id_of(&graph, "cycles/ring_a.ts");
    let ring_c = id_of(&graph, "cycles/ring_c.ts");

    assert_eq!(graph.dependencies(entry), [ring_a]);
    // ring_a is imported by the entry and by ring_c, which closes the ring.
    assert_eq!(graph.dependents(ring_a), [entry, ring_c]);
}

#[test]
fn a_ring_is_one_cycle_and_its_importer_is_not_part_of_it() {
    let graph = build(&["cycles/entry.ts"]);
    let cycles = graph.cycles();
    assert_eq!(cycles.len(), 1);
    assert_eq!(
        names(&graph, &cycles[0]),
        ["ring_a.ts", "ring_b.ts", "ring_c.ts"]
    );
}

#[test]
fn a_file_that_imports_itself_is_a_cycle() {
    let graph = build(&["self-import/self_import.ts"]);
    assert_eq!(graph.cycles(), vec![vec![FileId::new(0)]]);
}

#[test]
fn layers_run_dependencies_first_and_keep_a_cycle_together() {
    let graph = build(&["cycles/entry.ts"]);
    let layers: Vec<Vec<String>> = graph
        .layers()
        .iter()
        .map(|layer| names(&graph, layer))
        .collect();
    assert_eq!(
        layers,
        vec![
            vec!["leaf.ts"],
            vec!["ring_a.ts", "ring_b.ts", "ring_c.ts"],
            vec!["entry.ts"],
        ]
    );
}

#[test]
fn a_relative_entry_is_rejected() {
    let result = ModuleGraph::build(&[PathBuf::from("main.ts")], &ModuleResolver::new());
    assert!(matches!(result, Err(ModuleError::RelativeEntry(_))));
}

#[test]
fn a_missing_entry_is_a_read_error() {
    let result = ModuleGraph::build(&[fixture("basic/absent.ts")], &ModuleResolver::new());
    assert!(matches!(result, Err(ModuleError::Read { .. })));
}

#[test]
fn the_report_is_in_path_order_and_names_every_file() {
    let graph = build(&[
        "project-check/clean.ts",
        "project-check/type_error.ts",
        "project-check/syntax_error.ts",
    ]);
    assert!(graph.has_syntax_errors(id_of(&graph, "project-check/syntax_error.ts")));

    let report = check_project(&graph);
    assert_eq!(report.files.len(), 3);

    let ids: Vec<u32> = report
        .files
        .iter()
        .map(|file| file.file_id.index())
        .collect();
    assert_eq!(ids, [0, 1, 2], "reports are sorted by file id");

    // The entries above were given as clean, type_error, syntax_error. The report follows
    // path order, so the order they were given in does not show.
    let reported: Vec<FileId> = report.files.iter().map(|file| file.file_id).collect();
    assert_eq!(
        names(&graph, &reported),
        ["clean.ts", "syntax_error.ts", "type_error.ts"]
    );

    assert!(matches!(
        &report.files[0].outcome,
        FileOutcome::Checked(diagnostics) if diagnostics.is_empty()
    ));
    assert!(matches!(
        &report.files[1].outcome,
        FileOutcome::CheckFailed(_)
    ));
    assert!(matches!(
        &report.files[2].outcome,
        FileOutcome::Checked(diagnostics) if !diagnostics.is_empty()
    ));
    assert_eq!(report.failure_count(), 1);
    assert!(report.diagnostic_count() >= 1);
}

#[test]
fn dependency_declarations_are_in_the_graph_but_are_not_checked() {
    let graph = build(&["package-exports/app.ts"]);
    let declaration = id_of(
        &graph,
        "package-exports/node_modules/widget/types/index.d.ts",
    );
    assert!(!graph.is_checkable(declaration));

    let report = check_project(&graph);
    assert_eq!(report.files.len(), 1);
    assert_eq!(
        report.files[0].file_id,
        id_of(&graph, "package-exports/app.ts")
    );
}

#[test]
fn ids_follow_path_order_whatever_order_the_entries_were_given_in() {
    let from_entry = build(&["cycles/entry.ts"]);
    let reordered = build(&["cycles/ring_c.ts", "cycles/leaf.ts", "cycles/entry.ts"]);

    let expected = ["entry.ts", "leaf.ts", "ring_a.ts", "ring_b.ts", "ring_c.ts"];
    assert_eq!(names(&from_entry, &all_ids(&from_entry)), expected);
    assert_eq!(names(&reordered, &all_ids(&reordered)), expected);

    // The same ids mean the same edges and the same answers, wherever the walk started.
    for id in all_ids(&from_entry) {
        assert_eq!(from_entry.dependencies(id), reordered.dependencies(id));
        assert_eq!(from_entry.dependents(id), reordered.dependents(id));
    }
    assert_eq!(from_entry.layers(), reordered.layers());
    assert_eq!(from_entry.cycles(), reordered.cycles());
}

#[test]
fn an_entry_does_not_get_a_low_id_for_being_an_entry() {
    // ring_c is the only entry, and it sorts after everything it reaches.
    let graph = build(&["cycles/ring_c.ts"]);
    assert_eq!(
        names(&graph, &all_ids(&graph)),
        ["leaf.ts", "ring_a.ts", "ring_b.ts", "ring_c.ts"]
    );
    assert_eq!(id_of(&graph, "cycles/ring_c.ts"), FileId::new(3));
}

#[test]
fn adding_a_file_shifts_the_ids_after_it_and_nothing_else() {
    let dir = scratch_dir("path-order-shift");
    let a = dir.join("a.ts");
    let b = dir.join("b.ts");
    let c = dir.join("c.ts");
    fs::write(&a, "const x: number = 1;").expect("write should succeed");
    fs::write(&c, "const x: number = 1;").expect("write should succeed");

    let index = |graph: &ModuleGraph, path: &Path| {
        graph
            .files()
            .id(path)
            .expect("file is in the graph")
            .index()
    };

    let before = ModuleGraph::build(&[a.clone(), c.clone()], &ModuleResolver::new())
        .expect("scratch graph should build");
    assert_eq!((index(&before, &a), index(&before, &c)), (0, 1));

    fs::write(&b, "const x: number = 1;").expect("write should succeed");
    let after = ModuleGraph::build(&[a.clone(), b.clone(), c.clone()], &ModuleResolver::new())
        .expect("scratch graph should build");
    assert_eq!(
        (index(&after, &a), index(&after, &b), index(&after, &c)),
        (0, 1, 2),
        "an id is a place in the order, so it is never stored or put in a key"
    );

    let _ = fs::remove_dir_all(&dir);
}

// A scratch project for the tests that have to change files. Each test gets its own
// directory so they can run in parallel.
fn scratch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tsox-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch directory should be creatable");
    dir
}

// Moves a file's modification time ahead, so the test does not depend on how fine the
// filesystem's clock is.
fn push_mtime_forward(path: &Path, seconds: u64) {
    let file = OpenOptions::new()
        .write(true)
        .open(path)
        .expect("file should open for writing");
    file.set_modified(SystemTime::now() + Duration::from_secs(seconds))
        .expect("modification time should be settable");
}

#[test]
fn a_rewrite_with_the_same_bytes_is_not_a_change_but_new_bytes_are() {
    let dir = scratch_dir("changed-files");
    let path = dir.join("a.ts");
    fs::write(&path, "const a: number = 1;").expect("write should succeed");

    let graph = ModuleGraph::build(std::slice::from_ref(&path), &ModuleResolver::new())
        .expect("scratch graph should build");
    assert!(graph.changed_files().is_empty());

    push_mtime_forward(&path, 60);
    assert!(
        graph.changed_files().is_empty(),
        "a new timestamp over the same bytes is not a change"
    );

    // Same length, different content, and a timestamp that differs from both before.
    fs::write(&path, "const a: number = 2;").expect("write should succeed");
    push_mtime_forward(&path, 120);
    assert_eq!(graph.changed_files(), vec![FileId::new(0)]);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_components_come_dependencies_first() {
    let graph = build(&["cycles/entry.ts"]);
    let components = graph.sccs();
    let position = |name: &str| {
        let file = id_of(&graph, name);
        components
            .iter()
            .position(|members| members.contains(&file))
            .expect("every file is in a component")
    };
    assert!(position("cycles/leaf.ts") < position("cycles/ring_a.ts"));
    assert!(position("cycles/ring_a.ts") < position("cycles/entry.ts"));
    assert_eq!(position("cycles/ring_a.ts"), position("cycles/ring_b.ts"));
    assert_eq!(position("cycles/ring_b.ts"), position("cycles/ring_c.ts"));
}

#[test]
fn complete_project_reports_do_not_depend_on_entry_order() {
    let first = build(&["cycles/entry.ts"]);
    let second = build(&["cycles/ring_c.ts", "cycles/leaf.ts", "cycles/entry.ts"]);
    let first_report = check_project(&first);
    let second_report = check_project(&second);
    let signature = |graph: &ModuleGraph, report: &tsox::ProjectReport| {
        let mut rows = report
            .files
            .iter()
            .map(|file| {
                let path = graph
                    .files()
                    .path(file.file_id)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                (path, format!("{:?}:{:?}", file.outcome, file.reads))
            })
            .collect::<Vec<_>>();
        // FileIds are assigned from the entry list, so they are not an invariant when
        // callers supply the same entries in a different order. Compare by path.
        rows.sort_by(|left, right| left.0.cmp(&right.0));
        rows
    };
    assert_eq!(
        signature(&first, &first_report),
        signature(&second, &second_report)
    );
}

#[test]
fn a_deleted_file_counts_as_changed() {
    let dir = scratch_dir("deleted-file");
    let path = dir.join("a.ts");
    fs::write(&path, "const a: number = 1;").expect("write should succeed");

    let graph = ModuleGraph::build(std::slice::from_ref(&path), &ModuleResolver::new())
        .expect("scratch graph should build");
    fs::remove_file(&path).expect("remove should succeed");
    assert_eq!(graph.changed_files(), vec![FileId::new(0)]);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn export_star_as_creates_a_portable_namespace_value() {
    let graph = build(&["export-star-as/barrel.ts", "export-star-as/consumer.ts"]);
    let report = check_project(&graph);
    let consumer_id = id_of(&graph, "export-star-as/consumer.ts");
    let consumer = report
        .files
        .iter()
        .find(|file| file.file_id == consumer_id)
        .expect("consumer is reported");
    let FileOutcome::Checked(diagnostics) = &consumer.outcome else {
        panic!("consumer failed: {consumer:?}");
    };
    assert_eq!(
        diagnostics.len(),
        2,
        "the namespace should expose both values and types; only the two intentional mismatches should fail: {diagnostics:?}"
    );
}

#[test]
fn overloaded_exports_preserve_each_signature_across_files() {
    let graph = build(&["overloads/consumer.ts"]);
    let report = check_project(&graph);
    let consumer_id = id_of(&graph, "overloads/consumer.ts");
    let consumer = report
        .files
        .iter()
        .find(|file| file.file_id == consumer_id)
        .unwrap();
    let FileOutcome::Checked(diagnostics) = &consumer.outcome else {
        panic!("{consumer:?}");
    };
    assert_eq!(
        diagnostics.len(),
        1,
        "both overloads should be callable: {diagnostics:?}"
    );
}

#[test]
fn default_expression_exports_its_inferred_type() {
    let graph = build(&["default-expression/consumer.ts"]);
    let report = check_project(&graph);
    let consumer_id = id_of(&graph, "default-expression/consumer.ts");
    let consumer = report
        .files
        .iter()
        .find(|file| file.file_id == consumer_id)
        .unwrap();
    let FileOutcome::Checked(diagnostics) = &consumer.outcome else {
        panic!("{consumer:?}");
    };
    assert_eq!(
        diagnostics.len(),
        1,
        "the default should retain its numeric type: {diagnostics:?}"
    );
}

#[test]
fn anonymous_default_function_exports_its_inferred_signature() {
    let graph = build(&["default-function/consumer.ts"]);
    let report = check_project(&graph);
    let consumer_id = id_of(&graph, "default-function/consumer.ts");
    let consumer = report
        .files
        .iter()
        .find(|file| file.file_id == consumer_id)
        .unwrap();
    let FileOutcome::Checked(diagnostics) = &consumer.outcome else {
        panic!("{consumer:?}");
    };
    assert_eq!(
        diagnostics.len(),
        1,
        "the inferred callable signature should cross files: {diagnostics:?}"
    );
}

#[test]
fn graph_retains_static_dynamic_side_effect_reexport_and_import_equals_kinds() {
    use tsox::ModuleRequestKind as Kind;
    let graph = build(&["edge-kinds/entry.ts"]);
    assert_eq!(
        edge(&graph, "edge-kinds/entry.ts", "./polyfill").kind,
        Kind::SideEffectImport
    );
    assert_eq!(
        edge(&graph, "edge-kinds/entry.ts", "./lazy").kind,
        Kind::DynamicImport
    );
    assert_eq!(
        edge(&graph, "edge-kinds/entry.ts", "./exports").kind,
        Kind::ReExport
    );
    assert_eq!(
        edge(&graph, "edge-kinds/entry.ts", "./legacy").kind,
        Kind::ImportEquals
    );
}

// Problem: which edge kinds order the checking was only stated in a comment, and a dynamic
// import was kept out of `dependents`, so nothing could tell what a changed file affects.
// Picked: every kind but a dynamic import is a dependency, and a dynamic import is listed in
// `dynamic_dependents` instead, so each kind is in exactly one of the two lists.
#[test]
fn only_a_dynamic_import_is_kept_out_of_the_ordering_and_is_listed_apart() {
    let graph = build(&["edge-kinds/entry.ts"]);
    let entry = id_of(&graph, "edge-kinds/entry.ts");
    for ordered in ["polyfill", "exports", "legacy"] {
        let file = id_of(&graph, &format!("edge-kinds/{ordered}.ts"));
        assert_eq!(
            graph.dependents(file),
            [entry],
            "{ordered} orders the entry"
        );
        assert!(graph.dynamic_dependents(file).is_empty(), "{ordered}");
    }
    let lazy = id_of(&graph, "edge-kinds/lazy.ts");
    assert!(graph.dependents(lazy).is_empty());
    assert_eq!(graph.dynamic_dependents(lazy), [entry]);
    assert!(!graph.dependencies(entry).contains(&lazy));
    assert!(graph.dynamic_dependents(entry).is_empty());
}

#[test]
fn separate_ambient_module_blocks_union_their_export_tables() {
    let graph = build(&["ambient-merge/consumer.ts", "ambient-merge/modules.d.ts"]);
    let report = check_project(&graph);
    let consumer_id = id_of(&graph, "ambient-merge/consumer.ts");
    let consumer = report
        .files
        .iter()
        .find(|file| file.file_id == consumer_id)
        .unwrap();
    let FileOutcome::Checked(diagnostics) = &consumer.outcome else {
        panic!("{consumer:?}");
    };
    assert_eq!(
        diagnostics.len(),
        1,
        "both ambient blocks should be importable; only the intentional mismatch should fail: {diagnostics:?}"
    );
}

#[test]
fn module_augmentation_merges_into_the_resolved_target_module() {
    use tsox::ModuleRequestKind as Kind;
    let graph = build(&["augmentations/consumer.ts"]);
    assert_eq!(
        edge(&graph, "augmentations/augmentation.ts", "./base").kind,
        Kind::SideEffectImport
    );
    assert!(
        graph
            .edges(id_of(&graph, "augmentations/augmentation.ts"))
            .iter()
            .any(|edge| {
                edge.specifier == "./base"
                    && edge.kind == Kind::ModuleAugmentation
                    && edge.target.is_some()
            })
    );
    let report = check_project(&graph);
    let consumer_id = id_of(&graph, "augmentations/consumer.ts");
    let consumer = report
        .files
        .iter()
        .find(|file| file.file_id == consumer_id)
        .unwrap();
    let FileOutcome::Checked(diagnostics) = &consumer.outcome else {
        panic!("{consumer:?}");
    };
    assert_eq!(
        diagnostics.len(),
        1,
        "the added field is visible and still type-checked: {diagnostics:?}"
    );
}

#[test]
fn umd_global_namespace_is_available_to_scripts_but_not_external_modules() {
    let graph = build(&["umd/api.d.ts", "umd/script.ts", "umd/module.ts"]);
    let report = check_project(&graph);
    let script_id = id_of(&graph, "umd/script.ts");
    let script = report
        .files
        .iter()
        .find(|file| file.file_id == script_id)
        .unwrap();
    let FileOutcome::Checked(script_diagnostics) = &script.outcome else {
        panic!("{script:?}");
    };
    assert_eq!(
        script_diagnostics.len(),
        1,
        "Api is global in a script, with only the intentional property mismatch: {script_diagnostics:?}"
    );

    let module_id = id_of(&graph, "umd/module.ts");
    let module = report
        .files
        .iter()
        .find(|file| file.file_id == module_id)
        .unwrap();
    let FileOutcome::Checked(module_diagnostics) = &module.outcome else {
        panic!("{module:?}");
    };
    assert!(
        module_diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("Api")),
        "UMD globals should not leak into external modules: {module_diagnostics:?}"
    );
}

#[test]
fn umd_global_can_be_a_callable_export_assignment() {
    let graph = build(&[
        "umd-callable/api.d.ts",
        "umd-callable/script.ts",
        "umd-callable/module.ts",
    ]);
    let report = check_project(&graph);
    let script_id = id_of(&graph, "umd-callable/script.ts");
    let script = report
        .files
        .iter()
        .find(|file| file.file_id == script_id)
        .unwrap();
    let FileOutcome::Checked(diagnostics) = &script.outcome else {
        panic!("{script:?}");
    };
    assert_eq!(
        diagnostics.len(),
        1,
        "the callable UMD global should retain its signature: {diagnostics:?}"
    );

    let module_id = id_of(&graph, "umd-callable/module.ts");
    let module = report
        .files
        .iter()
        .find(|file| file.file_id == module_id)
        .unwrap();
    let FileOutcome::Checked(diagnostics) = &module.outcome else {
        panic!("{module:?}");
    };
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("Legacy")),
        "UMD globals must not leak into external modules: {diagnostics:?}"
    );
}

#[test]
fn export_assignment_object_exposes_named_properties() {
    let graph = build(&["export-assignment/consumer.ts"]);
    let report = check_project(&graph);
    let consumer_id = id_of(&graph, "export-assignment/consumer.ts");
    let consumer = report
        .files
        .iter()
        .find(|file| file.file_id == consumer_id)
        .unwrap();
    let FileOutcome::Checked(diagnostics) = &consumer.outcome else {
        panic!("{consumer:?}");
    };
    assert_eq!(
        diagnostics.len(),
        1,
        "named properties of export= object should be typed: {diagnostics:?}"
    );
}

#[test]
fn export_assignment_expression_preserves_known_object_properties() {
    let graph = build(&["export-assignment-expression/consumer.ts"]);
    let report = check_project(&graph);
    let consumer_id = id_of(&graph, "export-assignment-expression/consumer.ts");
    let consumer = report
        .files
        .iter()
        .find(|file| file.file_id == consumer_id)
        .unwrap();
    let FileOutcome::Checked(diagnostics) = &consumer.outcome else {
        panic!("{consumer:?}");
    };
    assert_eq!(
        diagnostics.len(),
        1,
        "known properties of an expression-valued export= should retain their types: {diagnostics:?}"
    );
}

#[test]
fn export_star_as_preserves_known_members_from_a_partial_module() {
    let graph = build(&["export-star-partial/consumer.ts"]);
    let report = check_project(&graph);
    let consumer_id = id_of(&graph, "export-star-partial/consumer.ts");
    let consumer = report
        .files
        .iter()
        .find(|file| file.file_id == consumer_id)
        .unwrap();
    let FileOutcome::Checked(diagnostics) = &consumer.outcome else {
        panic!("{consumer:?}");
    };
    assert_eq!(
        diagnostics.len(),
        1,
        "known namespace members should remain typed even while the source export list is partial: {diagnostics:?}"
    );
}
