#![cfg(feature = "module-resolution")]

use std::fs;
use std::path::{Path, PathBuf};

use tsox::{FileOutcome, ModuleGraph, ModuleResolver, ProjectConfig, Severity, check_project};

// `declare module "x" { ... }` in a module adds members to what `x` exports. The tests below
// say what is merged, that the result does not depend on the order the files are listed in,
// and what is left alone when the module being augmented cannot be read in full.

const CONFIG: &str = r#"{ "compilerOptions": { "strict": true, "moduleResolution": "bundler", "module": "esnext" }, "include": ["*.ts"] }"#;

fn run(test: &str, files: &[(&str, &str)], reversed: bool) -> Vec<(String, Vec<String>)> {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("augmentation-{test}"));
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

const BASE: &str = "export interface Foo { a: number }\nexport const k = 1;\n";
const AUG_B: &str = "import './a';\ndeclare module './a' { interface Foo { b: string } }\n";
const AUG_C: &str = "import './a';\ndeclare module './a' { interface Foo { c: boolean } }\n";

#[test]
fn two_files_augmenting_one_interface_both_contribute() {
    let result = run(
        "two-files",
        &[
            ("a.ts", BASE),
            ("aug1.ts", AUG_B),
            ("aug2.ts", AUG_C),
            (
                "use.ts",
                "import { Foo } from './a';\n\
                 export const ok: Foo = { a: 1, b: 's', c: true };\n\
                 export const missing: Foo = { a: 1, b: 's' };\n\
                 export const wrong: Foo = { a: 1, b: 2, c: true };\n",
            ),
        ],
        false,
    );
    assert_eq!(codes(&result, "use.ts"), ["Some(2322)", "Some(2741)"]);
}

#[test]
fn the_result_does_not_depend_on_the_order_the_files_are_listed_in() {
    let files: &[(&str, &str)] = &[
        ("a.ts", BASE),
        ("aug1.ts", AUG_B),
        ("aug2.ts", AUG_C),
        (
            "use.ts",
            "import { Foo } from './a';\n\
             export const ok: Foo = { a: 1, b: 's', c: true };\n\
             export const missing: Foo = { a: 1, b: 's' };\n\
             export const wrong: Foo = { a: 1, b: 2, c: true };\n",
        ),
    ];
    assert_eq!(run("order-a", files, false), run("order-b", files, true));
}

#[test]
fn an_augmentation_can_add_a_function_and_a_constant() {
    let result = run(
        "adds-exports",
        &[
            ("a.ts", BASE),
            (
                "aug.ts",
                "import './a';\ndeclare module './a' {\n  export function newFn(): number;\n  export const newConst: string;\n}\n",
            ),
            (
                "use.ts",
                "import { newFn, newConst } from './a';\n\
                 export const ok: number = newFn();\n\
                 export const wrong1: string = newFn();\n\
                 export const wrong2: number = newConst;\n",
            ),
        ],
        false,
    );
    assert_eq!(codes(&result, "use.ts"), ["Some(2322)", "Some(2322)"]);
}

const PACKAGE: &[(&str, &str)] = &[
    (
        "node_modules/pkg/package.json",
        r#"{ "name": "pkg", "version": "1.0.0", "types": "index.d.ts" }"#,
    ),
    (
        "node_modules/pkg/index.d.ts",
        "export interface Options { name: string }\nexport declare function make(o: Options): Options;\n",
    ),
];

const AUG_PKG: &str = "import 'pkg';\n\
declare module 'pkg' { interface Options { extra: number }\n  export function added(): string; }\n";

#[test]
fn a_keyed_package_keeps_its_own_members_next_to_the_augmentation() {
    let mut files: Vec<(&str, &str)> = PACKAGE.to_vec();
    files.push(("augpkg.ts", AUG_PKG));
    files.push((
        "use.ts",
        "import { Options, added } from 'pkg';\n\
         export const ok: Options = { name: 'n', extra: 1 };\n\
         export const missing: Options = { name: 'n' };\n\
         export const wrong: number = added();\n",
    ));
    let result = run("keyed-package", &files, false);
    assert_eq!(codes(&result, "use.ts"), ["Some(2322)", "Some(2741)"]);
}

#[test]
fn a_package_that_cannot_be_keyed_is_not_replaced_by_its_augmentation() {
    // No version in package.json, so the package's own declarations cannot be carried over.
    // The augmentation alone must not stand in for `Options`: its `name` would look missing.
    let files: Vec<(&str, &str)> = vec![
        (
            "node_modules/pkg/package.json",
            r#"{ "name": "pkg", "types": "index.d.ts" }"#,
        ),
        ("node_modules/pkg/index.d.ts", PACKAGE[1].1),
        ("augpkg.ts", AUG_PKG),
        (
            "use.ts",
            "import { Options } from 'pkg';\nexport const o: Options = { name: 'n', extra: 1 };\n",
        ),
    ];
    let result = run("unkeyed-package", &files, false);
    assert_eq!(codes(&result, "use.ts"), Vec::<String>::new());
}

#[test]
fn an_augmentation_that_adds_a_new_interface_is_visible() {
    let result = run(
        "new-interface",
        &[
            ("a.ts", BASE),
            (
                "aug.ts",
                "import './a';\ndeclare module './a' { interface Extra { z: number } }\n",
            ),
            (
                "use.ts",
                "import { Extra } from './a';\n\
                 export const ok: Extra = { z: 1 };\n\
                 export const wrong: Extra = { z: 's' };\n",
            ),
        ],
        false,
    );
    assert_eq!(codes(&result, "use.ts"), ["Some(2322)"]);
}

#[test]
fn an_interface_merged_into_a_class_adds_members_to_its_instances() {
    let files = [
        ("cls.ts", "export class Foo { a: number = 1; }\n"),
        (
            "aug.ts",
            "import './cls';\ndeclare module './cls' { interface Foo { extra: string } }\n",
        ),
        (
            "use.ts",
            "import { Foo } from './cls';\n\
             const f = new Foo();\n\
             export const ok: string = f.extra;\n\
             export const bad: number = f.extra;\n\
             export const own: number = f.a;\n",
        ),
    ];
    for reversed in [false, true] {
        let result = run("class-interface", &files, reversed);
        assert_eq!(codes(&result, "use.ts"), ["Some(2322)"], "{result:?}");
    }
}

#[test]
fn an_enum_augmentation_adds_members_and_keeps_the_originals() {
    let files = [
        ("en.ts", "export enum Color { Red, Green }\n"),
        (
            "aug.ts",
            "import './en';\ndeclare module './en' { enum Color { Blue = 2 } }\n",
        ),
        (
            "use.ts",
            "import { Color } from './en';\n\
             export const blue = Color.Blue;\n\
             export const red: Color = Color.Red;\n\
             export const bad: string = Color.Green;\n",
        ),
    ];
    for reversed in [false, true] {
        let result = run("enum-augment", &files, reversed);
        assert_eq!(codes(&result, "use.ts"), ["Some(2322)"], "{result:?}");
    }
}

#[test]
fn a_namespace_augmentation_adds_values_and_types() {
    let files = [
        (
            "nsmod.ts",
            "export namespace NS { export const a: number = 1; export interface I { x: number } }\n",
        ),
        (
            "nsaug.ts",
            "import './nsmod';\n\
             declare module './nsmod' { namespace NS { const k: string; interface J { y: number } } }\n",
        ),
        (
            "use.ts",
            "import { NS } from './nsmod';\n\
             export const a: number = NS.a;\n\
             export const k: number = NS.k;\n\
             export const s: string = NS.k;\n\
             export const j: NS.J = { y: 's' };\n",
        ),
    ];
    for reversed in [false, true] {
        let result = run("namespace-augment", &files, reversed);
        assert_eq!(
            codes(&result, "use.ts"),
            ["Some(2322)", "Some(2322)"],
            "{result:?}"
        );
    }
}
