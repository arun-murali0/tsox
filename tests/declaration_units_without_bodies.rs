//! A declaration file of a dependency is checked so that other files can import what it
//! declares; its errors are never reported. `UnitContext::without_body_checks` leaves out
//! the pass that finds those errors. That is only sound if nothing else a unit hands back
//! comes from that pass, so this runs each declaration file both ways and compares every
//! field of the output except the diagnostics. The corpus is every bundled library file, a
//! set of declaration forms picked to be the ones most likely to need the pass (ambient
//! initializers, enums, merged declarations, `export =`, overloads, `declare global`), every
//! `.d.ts` under tests/fixtures, and, when `TSOX_DTS_CORPUS` names a directory, every `.d.ts`
//! under it (the way to try a project's `node_modules`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tsox::{
    DeclKey, DeclProvider, FileId, ImportSource, ModuleKey, PortableDecl, UnitContext, UnitOutput,
    Worker,
};

// A project that knows nothing: every question is answered "nothing". A unit with no imports
// at all is checked as a single file; this one is checked as part of a project, in either
// mode of `collect_global_decls`.
struct Empty(bool);
impl DeclProvider for Empty {
    fn decl(&self, _: DeclKey) -> Option<Arc<PortableDecl>> {
        None
    }
}
impl ImportSource for Empty {
    fn export_keys(&self, _: &str, _: &str) -> Vec<DeclKey> {
        Vec::new()
    }
    fn collect_global_decls(&self) -> bool {
        self.0
    }
}

fn run(name: &str, source: &str, imports: Option<&dyn ImportSource>, skip: bool) -> UnitOutput {
    let mut ctx =
        UnitContext::new(FileId::ROOT, name, source).with_module(ModuleKey::project(name));
    if let Some(imports) = imports {
        ctx = ctx.with_imports(imports);
    }
    if skip {
        ctx = ctx.without_body_checks();
    }
    Worker::new().check_unit(&ctx).unwrap()
}

// Everything but the diagnostics, as one comparable value.
fn handed_on(output: &UnitOutput) -> String {
    format!(
        "incomplete={} reasons={:?} exports_complete={}\nexports={:?}\nnames={:?}\nambient={:?}\n\
         umd={:?}\nunresolved={:?}\naugmentations={:?}\nglobals={:?}\nreturn_reads={:?}",
        output.incomplete,
        output.incomplete_reasons,
        output.exports_complete,
        output.exports,
        output.export_names,
        output.ambient,
        output.umd_namespaces,
        output.unresolved_names,
        output.augmentations,
        output.globals,
        output.return_reads
    )
}

// Names of the sources whose output differs, with the first place it does.
static DIFFERENCES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

fn assert_same(name: &str, source: &str) -> usize {
    let mut compared = 0;
    for imports in [
        None,
        Some(&Empty(true) as &dyn ImportSource),
        Some(&Empty(false)),
    ] {
        let full = run(name, source, imports, false);
        let skipped = run(name, source, imports, true);
        let (a, b) = (handed_on(&skipped), handed_on(&full));
        if a != b {
            // Show where they first differ, not two multi-kilobyte dumps.
            let at = a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count();
            let window = |text: &str| {
                text.chars()
                    .skip(at.saturating_sub(60))
                    .take(260)
                    .collect::<String>()
            };
            DIFFERENCES.lock().unwrap().push(format!(
                "{name}: differs at {at}\n  skipped: ...{}\n  full:    ...{}",
                window(&a),
                window(&b)
            ));
            break;
        }
        compared += 1;
    }
    compared
}

fn no_differences() {
    let differences = DIFFERENCES.lock().unwrap();
    assert!(
        differences.is_empty(),
        "{} sources differ:\n{}",
        differences.len(),
        differences.join("\n")
    );
}

fn declaration_files(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().map(|entry| entry.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            declaration_files(&path, out);
        } else if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".d.ts"))
        {
            out.push(path);
        }
    }
}

fn corpus(root: &Path) -> usize {
    let mut files = Vec::new();
    declaration_files(root, &mut files);
    let mut compared = 0;
    for path in &files {
        let Ok(source) = std::fs::read_to_string(path) else {
            continue;
        };
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        compared += assert_same(&name, &source);
    }
    compared
}

#[test]
fn every_bundled_library_file_hands_on_the_same_without_the_pass() {
    let compared = corpus(&tsox_base::standard_lib::library_directory());
    no_differences();
    assert!(compared > 200, "compared only {compared} units");
}

#[test]
fn the_declaration_forms_that_might_need_the_pass_hand_on_the_same() {
    let forms: &[(&str, &str)] = &[
        (
            "ambient-values.d.ts",
            "declare const answer = 42;\ndeclare const name: 'x';\ndeclare const flag = true;\n\
             declare let n: number;\ndeclare var v: { a: string };\ndeclare function f(a: number): string;\n\
             declare function f(a: string): number;\ndeclare function g<T extends object>(x: T): T;\n",
        ),
        (
            "enums.d.ts",
            "declare enum A { X, Y = 5, Z }\ndeclare const enum B { P = 'p', Q = 'q' }\n\
             declare enum C { M = 1 << 2, N = M | 1 }\nexport {};\n",
        ),
        (
            "merging.d.ts",
            "declare function lib(): void;\ndeclare namespace lib { const version: string; interface Options { a?: number } }\n\
             interface Box<T> { value: T }\ninterface Box<T> { map<U>(f: (t: T) => U): Box<U> }\n\
             declare class Widget { static create(): Widget; private p: number; protected q: string; readonly r = 1; get g(): number; set g(v: number); }\n\
             declare namespace Widget { const registry: Map<string, Widget> }\n",
        ),
        (
            "classes.d.ts",
            "declare abstract class Base<T = string> { abstract run(x: T): void; static s: number; constructor(x?: T); }\n\
             declare class Derived extends Base<number> implements Iterable<number> { run(x: number): void; [Symbol.iterator](): Iterator<number>; }\n\
             interface Indexed { [key: string]: number; readonly [i: number]: number }\n",
        ),
        (
            "export-equals.d.ts",
            "declare function thing(): void;\ndeclare namespace thing { interface Config { x: number } const defaults: Config }\n\
             export = thing;\nexport as namespace Thing;\n",
        ),
        (
            "global-augmentation.d.ts",
            "export {};\ndeclare global {\n  interface Window { myApp: { version: string } }\n  var __DEV__: boolean;\n  function track(e: string): void;\n  const enum Flags { A = 1 }\n}\n\
             declare module 'some-module' { export const x: number; export default function d(): void; }\n\
             declare module '*.css' { const classes: { [k: string]: string }; export default classes; }\n",
        ),
        (
            "types.d.ts",
            "type Pick2<T, K extends keyof T> = { [P in K]: T[P] };\ntype Cond<T> = T extends string ? 'a' : T extends number ? 'b' : never;\n\
             type Tpl<T extends string> = `prefix-${T}-suffix`;\ntype Tup = [a: number, b?: string, ...rest: boolean[]];\n\
             declare const sym: unique symbol;\ndeclare const q: typeof sym;\ntype Rec<T> = T | Rec<T>[];\n\
             type Fn = new (...args: any[]) => object;\ninterface Callable { (x: number): string; new (x: string): Callable; prop: number }\n\
             export type { Pick2, Cond };\n",
        ),
        (
            "imports.d.ts",
            "import type { A } from './a';\nimport { B } from './b';\nimport C = require('./c');\n\
             export * from './d';\nexport { E as F } from './e';\nexport declare function use(a: A, b: B): C;\n\
             /// <reference types=\"node\" />\n/// <reference path=\"./x.d.ts\" />\n",
        ),
        (
            "unresolved.d.ts",
            "declare const x: Missing;\ninterface Uses { a: AlsoMissing; b: Array<Missing2> }\ndeclare function f(a: Nope): void;\n",
        ),
    ];
    let mut compared = 0;
    for (name, source) in forms {
        compared += assert_same(name, source);
    }
    no_differences();
    assert_eq!(compared, forms.len() * 3);
}

#[test]
fn every_declaration_file_under_the_test_fixtures_hands_on_the_same() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let compared = corpus(&root);
    eprintln!("{compared} units compared under tests/");
    no_differences();
}

#[test]
fn a_directory_named_by_tsox_dts_corpus_hands_on_the_same() {
    let Some(dir) = std::env::var_os("TSOX_DTS_CORPUS") else {
        return;
    };
    let compared = corpus(Path::new(&dir));
    eprintln!(
        "{compared} units compared under {}",
        Path::new(&dir).display()
    );
    no_differences();
    assert!(compared > 0, "TSOX_DTS_CORPUS holds no declaration files");
}
