#![cfg(feature = "module-resolution")]

use std::fs;
use std::path::Path;

use tsox::{FileOutcome, ModuleGraph, ModuleResolver, Severity, check_project};

// An interface's `extends` clause. It used to be ignored, so an interface had only its own members:
// an inherited member was a false "does not exist" error and a required inherited member was never
// required. That is how `interface Console extends console.Console {}` (in @types/node) made every
// `console.log` an error. Every expectation below was checked against `tsc` 6.0.3.

fn errors(test: &str, source: &str) -> Vec<String> {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("iface-extends-{test}"));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let path = root.join("a.ts");
    fs::write(&path, source).unwrap();
    let graph = ModuleGraph::build(&[path], &ModuleResolver::new())
        .expect("the project builds")
        .with_root(root);
    let report = check_project(&graph);
    let FileOutcome::Checked(diagnostics) = &report.files[0].outcome else {
        panic!("the file was not checked");
    };
    let mut lines: Vec<String> = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| format!("{:?}", d.code.tsc_code()))
        .collect();
    lines.sort();
    lines
}

#[test]
fn an_inherited_member_exists_and_is_typed() {
    let result = errors(
        "inherited",
        "interface A { x: number }\ninterface B extends A { y: number }\n\
         declare const b: B;\nexport const n: number = b.x;\nexport const s: string = b.x;\n",
    );
    assert_eq!(result, ["Some(2322)"]);
}

#[test]
fn a_required_inherited_member_is_required() {
    let result = errors(
        "required",
        "interface A { x: number }\ninterface B extends A { y: number }\n\
         export const ok: B = { x: 1, y: 2 };\nexport const missing: B = { y: 2 };\n",
    );
    assert_eq!(result, ["Some(2741)"]);
}

#[test]
fn several_bases_all_contribute() {
    let result = errors(
        "several",
        "interface A { x: number }\ninterface B { y: string }\n\
         interface C extends A, B { z: boolean }\n\
         export const ok: C = { x: 1, y: 's', z: true };\n\
         export const missing: C = { x: 1, y: 's' };\n",
    );
    assert_eq!(result, ["Some(2741)"]);
}

#[test]
fn a_generic_base_is_instantiated() {
    let result = errors(
        "generic",
        "interface Box<T> { v: T }\ninterface N extends Box<number> { w: number }\n\
         export const ok: N = { v: 1, w: 2 };\nexport const bad: N = { v: 's', w: 2 };\n",
    );
    assert_eq!(result, ["Some(2322)"]);
}

#[test]
fn an_own_member_wins_over_an_inherited_one() {
    let result = errors(
        "override",
        "interface A { x: number }\ninterface Over extends A { x: 1 }\n\
         export const ok: Over = { x: 1 };\n",
    );
    assert_eq!(result, Vec::<String>::new());
}

#[test]
fn a_base_that_cannot_be_read_leaves_the_interface_unmodelled_and_silent() {
    // A class base, an undeclared base and a circle are not modelled: no error is invented. (tsc
    // also reports the circle, TS2310; that one is missed.)
    let result = errors(
        "silent",
        "interface FromClass extends Klass { q: number }\nclass Klass { k = 1 }\n\
         declare const fc: FromClass;\nexport const kk = fc.k;\n\
         interface Loop1 extends Loop2 { a: number }\ninterface Loop2 extends Loop1 { b: number }\n\
         declare const l: Loop1;\nexport const lv = l.a;\n\
         interface Gone extends Missing { g: number }\ndeclare const gone: Gone;\n\
         export const gg = gone.g;\n",
    );
    assert_eq!(result, Vec::<String>::new());
}
