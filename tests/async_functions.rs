#![cfg(feature = "module-resolution")]

use std::fs;
use std::path::Path;

use tsox::{FileOutcome, ModuleGraph, ModuleResolver, Severity, check_project};

// An `async function` declaration with no return annotation used to have no signature, so it was
// never declared and its body was never checked: every error inside it went unreported and a
// call to it was unknown. It is now declared like a sync function and returns a Promise of what
// its body returns.

fn errors(test: &str, source: &str) -> Vec<String> {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("async-{test}"));
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
fn an_error_inside_an_async_function_body_is_reported() {
    let result = errors(
        "body",
        "export async function g() {\n  const y: string = 1;\n  return y;\n}\n",
    );
    assert_eq!(result, ["Some(2322)"]);
}

#[test]
fn a_call_returns_a_promise_of_what_the_body_returns() {
    let result = errors(
        "call",
        "export async function num() { return 1; }\n\
         export const a: number = num();\n\
         export const b: Promise<number> = num();\n",
    );
    assert_eq!(result, ["Some(2322)"]);
}

#[test]
fn await_unwraps_the_inferred_promise() {
    let result = errors(
        "await",
        "export async function num() { return 1; }\n\
         export async function via() {\n  const x: string = await num();\n  return x;\n}\n\
         export const ok: number = await num();\n",
    );
    assert_eq!(result, ["Some(2322)"]);
}

#[test]
fn several_returns_make_a_union_inside_the_promise() {
    let result = errors(
        "union",
        "export async function many(flag: boolean) {\n  if (flag) return 1;\n  return 's';\n}\n\
         export const d: string = await many(true);\n",
    );
    assert_eq!(result, ["Some(2322)"]);
}

#[test]
fn a_body_with_no_return_is_a_promise_of_void() {
    let result = errors(
        "void",
        "export async function nothing() {}\n\
         export const ok = nothing().then(() => 1);\n\
         export const bad: number = await nothing();\n",
    );
    assert_eq!(result, ["Some(2322)"]);
}
