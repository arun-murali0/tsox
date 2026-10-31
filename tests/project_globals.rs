#![cfg(feature = "module-resolution")]

use std::fs;
use std::path::{Path, PathBuf};

use tsox::{FileOutcome, ModuleGraph, ModuleResolver, Severity, check_project};

// A type, function or variable a file declares globally (a top-level declaration of a script
// file, or a member of `declare global { }`) is visible to every other file of the project.
// Before, each use was reported as "Cannot find name".

fn run(test: &str, files: &[(&str, &str)], reversed: bool) -> Vec<(String, Vec<String>)> {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("globals-{test}"));
    let _ = fs::remove_dir_all(&root);
    let mut paths: Vec<PathBuf> = Vec::new();
    for (name, source) in files {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, source).unwrap();
        paths.push(path);
    }
    if reversed {
        paths.reverse();
    }
    let graph = ModuleGraph::build(&paths, &ModuleResolver::new())
        .expect("the project builds")
        .with_root(root);
    let report = check_project(&graph);
    let mut out = Vec::new();
    for file in &report.files {
        let path = graph.files().path(file.file_id).unwrap();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let FileOutcome::Checked(diagnostics) = &file.outcome else {
            panic!("{name} was not checked: {:?}", file.outcome);
        };
        let mut lines: Vec<String> = diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .map(|d| format!("{:?} {}", d.code.tsc_code(), d.message))
            .collect();
        lines.sort();
        out.push((name, lines));
    }
    out.sort();
    out
}

fn errors_of(result: &[(String, Vec<String>)], file: &str) -> Vec<String> {
    result
        .iter()
        .find(|(name, _)| name == file)
        .unwrap_or_else(|| panic!("{file} is not in the report"))
        .1
        .clone()
}

fn codes_of(result: &[(String, Vec<String>)], file: &str) -> Vec<String> {
    errors_of(result, file)
        .iter()
        .map(|line| line.split(' ').next().unwrap().to_owned())
        .collect()
}

const SCRIPT: &str = "interface Foo { x: number }\n\
declare const gv: Foo;\n\
declare type Id = string;\n\
declare function ff(a: number): string;\n\
declare function ff(a: string): number;\n";

const GLOBAL_BLOCK: &str = "export {};\n\
declare global { interface Bar { y: string } var gb: Bar; }\n";

#[test]
fn a_script_files_declarations_are_visible_to_other_files() {
    let result = run(
        "script-visible",
        &[
            ("a.d.ts", SCRIPT),
            (
                "c.ts",
                "export const f: Foo = { x: 1 };\n\
                 export const h: Id = 'a';\n\
                 export const n = gv.x;\n\
                 export const s: string = ff(1);\n\
                 export const k: number = ff('a');\n",
            ),
        ],
        false,
    );
    assert_eq!(errors_of(&result, "c.ts"), Vec::<String>::new());
}

#[test]
fn declare_global_members_are_visible_to_other_files_and_to_their_own() {
    let result = run(
        "declare-global",
        &[
            (
                "b.ts",
                &format!("{GLOBAL_BLOCK}export const own: Bar = {{ y: 's' }};\n"),
            ),
            (
                "c.ts",
                "export const g: Bar = { y: 's' };\nexport const m = gb.y;\n",
            ),
        ],
        false,
    );
    assert_eq!(errors_of(&result, "b.ts"), Vec::<String>::new());
    assert_eq!(errors_of(&result, "c.ts"), Vec::<String>::new());
}

#[test]
fn a_wrong_use_of_a_global_is_still_reported() {
    let result = run(
        "wrong-use",
        &[
            ("a.d.ts", SCRIPT),
            ("b.ts", GLOBAL_BLOCK),
            (
                "c.ts",
                "export const a: Foo = { x: 's' };\n\
                 export const b = gv.nope;\n\
                 export const c: Bar = { y: 1 };\n\
                 export const d = gb.zzz;\n\
                 export const e: number = ff(1);\n",
            ),
        ],
        false,
    );
    assert_eq!(
        codes_of(&result, "c.ts"),
        [
            "Some(2322)",
            "Some(2322)",
            "Some(2322)",
            "Some(2339)",
            "Some(2339)"
        ]
    );
}

#[test]
fn an_interface_declared_in_two_files_merges() {
    let result = run(
        "merge",
        &[
            ("m1.d.ts", "interface Merged { one: number }\n"),
            ("m2.d.ts", "interface Merged { two: string }\n"),
            (
                "c.ts",
                "export const ok: Merged = { one: 1, two: 't' };\n\
                 export const bad: Merged = { one: 1 };\n",
            ),
        ],
        false,
    );
    assert_eq!(codes_of(&result, "c.ts"), ["Some(2741)"]);
}

#[test]
fn the_result_does_not_depend_on_the_order_the_files_are_listed_in() {
    let files: &[(&str, &str)] = &[
        (
            "m1.d.ts",
            "interface Merged { one: number }\ndeclare const g1: Merged;\n",
        ),
        ("m2.d.ts", "interface Merged { two: string }\n"),
        ("a.d.ts", SCRIPT),
        ("b.ts", GLOBAL_BLOCK),
        (
            "c.ts",
            "export const a: Foo = { x: 's' };\n\
             export const b: Merged = { one: 1 };\n\
             export const c = gb.zzz;\n\
             export const d: Missing = 1;\n\
             export const e = g1.two;\n",
        ),
    ];
    let forward = run("order-forward", files, false);
    let reversed = run("order-reversed", files, true);
    assert_eq!(forward, reversed);
    assert!(!errors_of(&forward, "c.ts").is_empty());
}

#[test]
fn a_name_nothing_declares_is_still_missing() {
    let result = run(
        "still-missing",
        &[
            ("a.d.ts", SCRIPT),
            (
                "c.ts",
                "export const a: Nope = 1;\nexport const b = nope;\n",
            ),
        ],
        false,
    );
    assert_eq!(codes_of(&result, "c.ts"), ["Some(2304)", "Some(2304)"]);
}

#[test]
fn a_modules_own_top_level_declarations_are_not_global() {
    let result = run(
        "module-private",
        &[
            (
                "m.ts",
                "interface Hidden { x: number }\nexport const v = 1;\n",
            ),
            ("c.ts", "export const a: Hidden = { x: 1 };\n"),
        ],
        false,
    );
    assert_eq!(codes_of(&result, "c.ts"), ["Some(2304)"]);
}

#[test]
fn a_name_the_library_provides_is_not_replaced_by_a_project_declaration() {
    // The library's `Window` has far more members than this file adds. Binding the project's
    // declaration in its place would make `location` look missing.
    let result = run(
        "lib-name",
        &[
            ("w.d.ts", "interface Window { app: number }\n"),
            (
                "c.ts",
                "export function f(w: Window) { return w.location; }\n",
            ),
        ],
        false,
    );
    assert_eq!(errors_of(&result, "c.ts"), Vec::<String>::new());
}

#[test]
fn a_global_that_could_not_be_carried_over_is_not_reported_missing_but_others_still_are() {
    // `shape` is declared in a script, but its type (`Missing.T`) cannot be carried over, so
    // its type is unknown: using it is no missing name. An unrelated missing name next to it is
    // still reported, since one unmodelled global must not silence the whole project.
    let result = run(
        "unmodelled",
        &[
            ("a.ts", "declare const shape: Missing.T;\n"),
            (
                "c.ts",
                "export const a = shape;\nexport const b = nothing;\n",
            ),
        ],
        false,
    );
    assert_eq!(codes_of(&result, "c.ts"), ["Some(2304)"]);
    assert!(errors_of(&result, "c.ts")[0].contains("nothing"));
}

#[test]
fn a_file_that_declares_a_global_itself_sees_the_other_files_parts_of_it() {
    // `G` is one interface of both members, in either file and in either listing order.
    // Before, each file saw only its own member and reported the other's as unknown.
    let files = [
        (
            "one.ts",
            "interface G { a: number }\n\
             const ok: G = { a: 1, b: 2 };\n\
             const missing: G = { a: 1 };\n",
        ),
        (
            "two.ts",
            "export {};\ndeclare global { interface G { b: number } }\n\
             export const ok2: G = { a: 1, b: 2 };\n\
             export const bad: G = { a: 's', b: 2 };\n",
        ),
    ];
    for reversed in [false, true] {
        let result = run("same-file-merge", &files, reversed);
        assert_eq!(codes_of(&result, "one.ts"), ["Some(2741)"], "{result:?}");
        assert_eq!(codes_of(&result, "two.ts"), ["Some(2322)"], "{result:?}");
    }
}
