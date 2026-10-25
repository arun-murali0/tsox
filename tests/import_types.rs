#![cfg(feature = "module-resolution")]

use std::fs;
use std::path::{Path, PathBuf};

use tsox::{FileOutcome, ModuleGraph, ModuleResolver, ProjectConfig, Severity, check_project};

// `typeof import("x")`, `import("x").Name`, `import("x")` and `await`. Before, each was an
// unresolved annotation or an unchecked expression, so nothing written with them was checked.

const CONFIG: &str = r#"{ "compilerOptions": { "strict": true, "moduleResolution": "bundler", "module": "esnext" }, "include": ["*.ts"] }"#;

fn run(test: &str, files: &[(&str, &str)], reversed: bool) -> Vec<(String, Vec<String>)> {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("importtypes-{test}"));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let mut all: Vec<(&str, &str)> = files.to_vec();
    all.push(("tsconfig.json", CONFIG));
    let mut entries: Vec<PathBuf> = Vec::new();
    for (name, text) in &all {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        if name.ends_with(".ts") && !name.contains("node_modules") {
            entries.push(path);
        }
    }
    if reversed {
        entries.reverse();
    }
    let config = ProjectConfig::load(root.join("tsconfig.json")).expect("the config loads");
    let resolver =
        ModuleResolver::with_tsconfig(config.config_file()).with_node_next(config.uses_node_next());
    let graph = ModuleGraph::build(&entries, &resolver)
        .expect("the project builds")
        .with_root(&root)
        .with_config(&config);
    let report = check_project(&graph);
    let mut out = Vec::new();
    for file in &report.files {
        let name = graph
            .files()
            .path(file.file_id)
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let FileOutcome::Checked(diagnostics) = &file.outcome else {
            continue;
        };
        let mut lines: Vec<String> = diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .map(|d| format!("{:?}", d.code.tsc_code()))
            .collect();
        lines.sort();
        out.push((name, lines));
    }
    out.sort();
    out
}

fn codes(result: &[(String, Vec<String>)], file: &str) -> Vec<String> {
    result
        .iter()
        .find(|(name, _)| name == file)
        .unwrap_or_else(|| panic!("{file} is not in the report"))
        .1
        .clone()
}

const MODULE: &str = "export const k = 1;\n\
export interface Foo { a: number }\n\
export function f(): string { return 's'; }\n";

#[test]
fn typeof_import_is_the_module_object() {
    let result = run(
        "typeof",
        &[
            ("a.ts", MODULE),
            (
                "use.ts",
                "declare const x: typeof import('./a');\n\
                 export const ok: number = x.k;\n\
                 export const w1: string = x.k;\n\
                 export const w2: number = x.f();\n\
                 export const w3 = x.missing;\n",
            ),
        ],
        false,
    );
    assert_eq!(
        codes(&result, "use.ts"),
        ["Some(2322)", "Some(2322)", "Some(2339)"]
    );
}

#[test]
fn an_import_type_names_a_type_of_the_module() {
    let result = run(
        "named",
        &[
            ("a.ts", MODULE),
            (
                "use.ts",
                "export const ok: import('./a').Foo = { a: 1 };\n\
                 export type F = import('./a').Foo;\n\
                 export const w1: F = { a: 's' };\n\
                 export const w2: import('./a').Foo = {};\n",
            ),
        ],
        false,
    );
    assert_eq!(codes(&result, "use.ts"), ["Some(2322)", "Some(2741)"]);
}

#[test]
fn a_dynamic_import_the_file_also_imports_as_a_type_is_a_promise_of_the_module() {
    let result = run(
        "dynamic-typed",
        &[
            ("a.ts", MODULE),
            (
                "use.ts",
                "import type { Foo } from './a';\n\
                 export const p = import('./a');\n\
                 export const w0: string = p;\n\
                 export async function g(): Promise<void> {\n\
                 \x20 const m = await p;\n\
                 \x20 const w1: string = m.k;\n\
                 \x20 const w2 = m.nope;\n\
                 }\n\
                 export const top: Foo = { a: 1 };\n",
            ),
        ],
        false,
    );
    assert_eq!(
        codes(&result, "use.ts"),
        ["Some(2322)", "Some(2322)", "Some(2339)"]
    );
}

#[test]
fn a_lazy_import_of_a_module_the_file_does_not_otherwise_import_is_read() {
    // The check waits for the module a lazy import loads whenever that cannot make a cycle, so
    // what it exports is known and the answer is the same in every run.
    let files: &[(&str, &str)] = &[
        ("a.ts", MODULE),
        (
            "use.ts",
            "export async function g(): Promise<void> {\n\
             \x20 const m = await import('./a');\n\
             \x20 const w: string = m.k;\n\
             }\n",
        ),
    ];
    let forward = run("lazy-a", files, false);
    let reversed = run("lazy-b", files, true);
    assert_eq!(forward, reversed);
    assert_eq!(codes(&forward, "use.ts"), ["Some(2322)"]);
}

#[test]
fn a_lazy_import_that_would_close_a_cycle_is_not_read() {
    // `b` imports `a`, so `a` loading `b` lazily would make `a` and `b` wait for each other.
    // That edge is left out of the ordering and its module is left unread; nothing is reported.
    let files: &[(&str, &str)] = &[
        (
            "a.ts",
            "export const ka = 1;\n\
             export async function load() {\n\
             \x20 const m = await import('./b');\n\
             \x20 const w: string = m.kb;\n\
             \x20 return w;\n\
             }\n",
        ),
        ("b.ts", "import { ka } from './a';\nexport const kb = ka;\n"),
    ];
    let forward = run("cycle-a", files, false);
    let reversed = run("cycle-b", files, true);
    assert_eq!(forward, reversed);
    assert_eq!(codes(&forward, "a.ts"), Vec::<String>::new());
    assert_eq!(codes(&forward, "b.ts"), Vec::<String>::new());
}

#[test]
fn of_two_files_that_load_each_other_the_same_edge_is_kept_in_every_listing_order() {
    // Neither imports the other any other way. Edges are taken in path order, so `x` reading
    // `y` is kept and `y` reading `x` is not, whichever order the files were listed in.
    let files: &[(&str, &str)] = &[
        (
            "x.ts",
            "export const kx = 1;\n\
             export async function lx() {\n\
             \x20 const m = await import('./y');\n\
             \x20 const w: string = m.ky;\n\
             \x20 return w;\n\
             }\n",
        ),
        (
            "y.ts",
            "export const ky = 1;\n\
             export async function ly() {\n\
             \x20 const m = await import('./x');\n\
             \x20 const w: string = m.kx;\n\
             \x20 return w;\n\
             }\n",
        ),
    ];
    let forward = run("mutual-a", files, false);
    let reversed = run("mutual-b", files, true);
    assert_eq!(forward, reversed);
    assert_eq!(codes(&forward, "x.ts"), ["Some(2322)"]);
    assert_eq!(codes(&forward, "y.ts"), Vec::<String>::new());
}

#[test]
fn the_result_does_not_depend_on_the_order_the_files_are_listed_in() {
    let files: &[(&str, &str)] = &[
        ("a.ts", MODULE),
        (
            "use.ts",
            "declare const x: typeof import('./a');\n\
             export const w1: string = x.k;\n\
             export const w2: import('./a').Foo = {};\n",
        ),
    ];
    assert_eq!(run("order-a", files, false), run("order-b", files, true));
}

#[test]
fn a_module_that_is_not_known_in_full_is_left_unresolved() {
    // `C` extends a class that does not exist, a shape not yet checked, so the module's
    // exports are partial. `typeof import` must not become an object missing members.
    let result = run(
        "partial",
        &[
            (
                "a.ts",
                "export const k = 1;\nexport class C extends Missing { z = 1; }\n",
            ),
            (
                "use.ts",
                "declare const x: typeof import('./a');\nexport const n: number = x.k;\nexport const o = x.other;\n",
            ),
        ],
        false,
    );
    assert_eq!(codes(&result, "use.ts"), Vec::<String>::new());
}

#[test]
fn await_unwraps_a_promise_and_leaves_other_values_alone() {
    let result = run(
        "await",
        &[(
            "use.ts",
            "declare const p: Promise<number>;\n\
             export async function g(): Promise<void> {\n\
             \x20 const a: number = await p;\n\
             \x20 const b: string = await p;\n\
             \x20 const c: number = await 1;\n\
             \x20 const d: string = await 1;\n\
             }\n",
        )],
        false,
    );
    assert_eq!(codes(&result, "use.ts"), ["Some(2322)", "Some(2322)"]);
}
