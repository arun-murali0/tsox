#![cfg(feature = "module-resolution")]

use std::fs;
use std::path::Path;

use tsox::{FileOutcome, ModuleGraph, ModuleResolver, Severity, check_project};

// The methods of arrays and promises. Before, an array only had `length` and a Promise had
// nothing, so `arr.map(f)`, `arr.push(x)` and `p.then(f)` were false "Property does not exist"
// errors. Methods whose real signature needs generic inference are typed loosely (callbacks
// and results `any`); the rest are typed from the element type.

fn errors(test: &str, source: &str) -> Vec<String> {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("library-{test}"));
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

const NUMBERS: &str = "declare const arr: number[];\ndeclare const p: Promise<number>;\n";

fn with(body: &str) -> String {
    format!("{NUMBERS}{body}")
}

#[test]
fn the_common_array_methods_exist() {
    let result = errors(
        "array-methods",
        &with(
            "export const a1 = arr.map((x) => x + 1);\n\
             export const a2 = arr.filter((x) => x > 1);\n\
             export const a3 = arr.indexOf(1);\n\
             export const a4 = arr.includes(1);\n\
             export const a5 = arr.slice(1);\n\
             export const a6 = arr.concat([1]);\n\
             export const a7 = arr.reduce((a, b) => a + b, 0);\n\
             export const a8 = arr.some((x) => x > 1);\n\
             export const a9 = arr.every((x) => x > 1);\n\
             export const b1 = arr.join(',');\n\
             export const b2 = arr.push(1, 2);\n\
             export const b3 = arr.find((x) => x > 1);\n\
             export const b4 = arr.sort();\n\
             export const b5 = arr.forEach((x) => x);\n\
             export const b6 = arr.flat();\n\
             export const b7 = arr.findIndex((x) => x > 1);\n\
             export const b8 = arr.flatMap((x) => [x]);\n\
             export const b9 = arr.splice(0, 1);\n\
             export const c1 = arr.reverse();\n\
             export const c2 = arr.fill(0);\n\
             export const c3 = arr.pop();\n\
             export const c4 = arr.shift();\n\
             export const c5 = arr.unshift(1);\n\
             export const c6 = arr.lastIndexOf(1);\n\
             export const c7 = arr.toString();\n",
        ),
    );
    assert_eq!(result, Vec::<String>::new());
}

#[test]
fn promise_then_catch_and_finally_exist() {
    let result = errors(
        "promise-methods",
        &with(
            "export const q1 = p.then((v) => v + 1);\n\
             export const q2 = p.catch(() => 0);\n\
             export const q3 = p.finally(() => {});\n\
             export const q4 = p.then((v) => v, () => 0);\n",
        ),
    );
    assert_eq!(result, Vec::<String>::new());
}

#[test]
fn a_wrong_use_of_a_typed_method_is_still_reported() {
    let result = errors(
        "array-wrong",
        &with(
            "export const w1 = arr.push('s');\n\
             export const w2: string = arr.indexOf(1);\n\
             export const w3 = arr.join(1);\n\
             export const w4: string = arr.pop();\n\
             export const w5: string = arr.includes(1);\n\
             export const w6 = arr.fill('s');\n",
        ),
    );
    assert_eq!(
        result,
        [
            "Some(2322)",
            "Some(2322)",
            "Some(2322)",
            "Some(2345)",
            "Some(2345)",
            "Some(2345)"
        ]
    );
}

#[test]
fn a_member_that_does_not_exist_is_still_reported() {
    let result = errors(
        "missing-member",
        &with("export const m1 = arr.nope;\nexport const m2 = p.nope;\n"),
    );
    assert_eq!(result, ["Some(2339)", "Some(2339)"]);
}

#[test]
fn element_types_flow_through_the_methods_that_keep_them() {
    // `slice` and `concat` keep the element type, so `string` from them is wrong; `map` and
    // `filter` return `any[]` and say nothing about their elements.
    let result = errors(
        "elements",
        &with(
            "export const e1: number[] = arr.slice(1);\n\
             export const e2: string[] = arr.slice(1);\n\
             export const e3: string[] = arr.map((x) => String(x));\n\
             export const e4: string[] = arr.filter((x) => x > 1);\n",
        ),
    );
    assert_eq!(result, ["Some(2322)"]);
}

#[test]
fn methods_on_a_string_array_use_the_string_element() {
    let result = errors(
        "string-array",
        "declare const names: string[];\n\
         export const n1 = names.join('-');\n\
         export const n2: number = names.push('a');\n\
         export const n3 = names.push(1);\n",
    );
    assert_eq!(result, ["Some(2345)"]);
}
