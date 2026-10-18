#![cfg(feature = "module-resolution")]

use std::fs;
use std::path::{Path, PathBuf};

use tsox::{FileId, ModuleGraph, ModuleResolver};

// The shape of the module graph: which imports order files, how cycles are found and
// keyed. Every test writes a small project into its own directory under the target tree.

fn write(test: &str, files: &[(&str, &str)]) -> (PathBuf, Vec<PathBuf>) {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("graph-shape-{test}"));
    let _ = fs::remove_dir_all(&root);
    let mut paths = Vec::new();
    for (name, source) in files {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, source).unwrap();
        paths.push(path);
    }
    (root, paths)
}

fn build(test: &str, files: &[(&str, &str)]) -> ModuleGraph {
    let (root, paths) = write(test, files);
    ModuleGraph::build(&paths, &ModuleResolver::new())
        .expect("the project builds")
        .with_root(root)
}

fn all_ids(graph: &ModuleGraph) -> impl Iterator<Item = FileId> {
    (0..graph.files().len() as u32).map(FileId::new)
}

fn id(graph: &ModuleGraph, name: &str) -> FileId {
    all_ids(graph)
        .find(|candidate| {
            graph
                .files()
                .path(*candidate)
                .is_some_and(|path| path.file_name().is_some_and(|file| file == name))
        })
        .unwrap_or_else(|| panic!("no file named {name}"))
}

// A dynamic import used to be an ordinary dependency, so two files that import each other
// only through `import()` were reported as a cycle and checked as one.
#[test]
fn a_dynamic_import_back_to_the_importer_is_not_a_cycle() {
    let graph = build(
        "dynamic-not-a-cycle",
        &[
            ("a.ts", "import { b } from './b';\nexport const a = b;"),
            (
                "b.ts",
                "export const b = 1;\nexport const lazy = () => import('./a');",
            ),
        ],
    );
    assert!(graph.cycles().is_empty(), "{:?}", graph.cycles());
    let (a, b) = (id(&graph, "a.ts"), id(&graph, "b.ts"));
    assert_eq!(graph.dependencies(a), &[b]);
    assert!(graph.dependencies(b).is_empty());
    // b comes first, and the dynamic edge back to a does not change that.
    assert_eq!(graph.layers(), vec![vec![b], vec![a]]);
}

#[test]
fn a_file_imported_both_ways_is_still_a_dependency() {
    let graph = build(
        "static-and-dynamic",
        &[
            ("a.ts", "export const a = 1;"),
            (
                "b.ts",
                "import { a } from './a';\nexport const b = a;\nexport const lazy = () => import('./a');",
            ),
        ],
    );
    assert_eq!(
        graph.dependencies(id(&graph, "b.ts")),
        &[id(&graph, "a.ts")]
    );
}

// A dynamic import that is the only way in still brings the file into the project.
#[test]
fn a_file_reached_only_by_a_dynamic_import_is_found() {
    let (root, paths) = write(
        "dynamic-only-found",
        &[
            ("entry.ts", "export const load = () => import('./late');"),
            ("late.ts", "export const late = 1;"),
        ],
    );
    let entry = vec![paths[0].clone()];
    let graph = ModuleGraph::build(&entry, &ModuleResolver::new())
        .expect("the project builds")
        .with_root(root);
    assert!(all_ids(&graph).any(|file| {
        graph
            .files()
            .path(file)
            .is_some_and(|path| path.ends_with("late.ts"))
    }));
}

#[test]
fn components_are_listed_dependencies_first_and_scc_of_agrees() {
    let graph = build(
        "components",
        &[
            ("a.ts", "import './b';"),
            ("b.ts", "import './c';"),
            ("c.ts", "import './b';"),
            ("d.ts", "import './a';"),
        ],
    );
    let (a, b, c, d) = (
        id(&graph, "a.ts"),
        id(&graph, "b.ts"),
        id(&graph, "c.ts"),
        id(&graph, "d.ts"),
    );
    let sccs = graph.sccs();
    assert_eq!(sccs.len(), 3, "{sccs:?}");
    let position = |file| graph.scc_of(file);
    // b and c are one component, and everything that imports it comes after it.
    assert_eq!(position(b), position(c));
    assert!(position(b) < position(a));
    assert!(position(a) < position(d));
    assert_eq!(sccs[position(b)], {
        let mut members = vec![b, c];
        members.sort();
        members
    });
    assert_eq!(graph.cycles(), vec![sccs[position(b)].clone()]);
}

// The key of a cycle comes from its members' module keys: it does not depend on the order
// the members are given in, nor on where the project lives, and it changes with membership.
#[test]
fn a_cycle_key_ignores_member_order_and_project_location() {
    let files: &[(&str, &str)] = &[
        ("a.ts", "import './b';"),
        ("b.ts", "import './c';"),
        ("c.ts", "import './a';"),
        ("d.ts", "import './a';\nimport './e';"),
        ("e.ts", "import './d';"),
    ];
    let first = build("cycle-key-one", files);
    let second = build("cycle-key-two-elsewhere", files);

    let members = |graph: &ModuleGraph, names: &[&str]| -> Vec<FileId> {
        names.iter().map(|name| id(graph, name)).collect()
    };
    let abc = first
        .cycle_key(&members(&first, &["a.ts", "b.ts", "c.ts"]))
        .unwrap();
    let cba = first
        .cycle_key(&members(&first, &["c.ts", "b.ts", "a.ts"]))
        .unwrap();
    assert_eq!(abc, cba);

    let moved = second
        .cycle_key(&members(&second, &["a.ts", "b.ts", "c.ts"]))
        .unwrap();
    assert_eq!(
        abc, moved,
        "the same files under another root are the same cycle"
    );

    let de = first
        .cycle_key(&members(&first, &["d.ts", "e.ts"]))
        .unwrap();
    assert_ne!(abc, de);
    let ab = first
        .cycle_key(&members(&first, &["a.ts", "b.ts"]))
        .unwrap();
    assert_ne!(abc, ab, "a different set of members is a different cycle");
}
