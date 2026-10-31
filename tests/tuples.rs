#![cfg(feature = "module-resolution")]

use std::fs;
use std::path::{Path, PathBuf};

use tsox::{FileOutcome, ModuleGraph, ModuleResolver, Severity, check_project};

// Tuple types: `[string, number]`, optional elements (`number?`), a trailing rest element
// (`...number[]`), and what each of them does for assignability, literals, indexing,
// destructuring, `length`, array methods, generic inference and the export of a tuple from
// one file to another. Each test says what tsc does and checks that this checker agrees.

fn check(test: &str, files: &[(&str, &str)]) -> Vec<(String, String, String)> {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("tuples-{test}"));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let mut paths: Vec<PathBuf> = Vec::new();
    for (name, source) in files {
        let path = root.join(name);
        fs::write(&path, source).unwrap();
        paths.push(path);
    }
    let graph = ModuleGraph::build(&paths, &ModuleResolver::new())
        .expect("the project builds")
        .with_root(root);
    let report = check_project(&graph);
    let mut found = Vec::new();
    for file in &report.files {
        let FileOutcome::Checked(diagnostics) = &file.outcome else {
            panic!("a file was not checked");
        };
        for d in diagnostics.iter().filter(|d| d.severity == Severity::Error) {
            let name = Path::new(&d.file_name)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            found.push((name, format!("{:?}", d.code.tsc_code()), d.message.clone()));
        }
    }
    found.sort();
    found
}

fn codes(test: &str, source: &str) -> Vec<String> {
    check(test, &[("a.ts", source)])
        .into_iter()
        .map(|(_, code, _)| code)
        .collect()
}

fn messages(test: &str, source: &str) -> Vec<String> {
    check(test, &[("a.ts", source)])
        .into_iter()
        .map(|(_, _, message)| message)
        .collect()
}

// ---- assignability of a literal -------------------------------------------------------

#[test]
fn an_array_literal_with_the_right_elements_is_a_tuple() {
    assert!(
        codes(
            "literal-ok",
            "export const p: [string, number] = [\"a\", 1];\n"
        )
        .is_empty()
    );
}

#[test]
fn a_wrong_element_type_is_reported() {
    assert_eq!(
        codes(
            "wrong-element",
            "export const p: [string, number] = [\"a\", \"b\"];\n"
        ),
        ["Some(2322)"]
    );
}

#[test]
fn too_many_elements_is_reported_and_names_the_tuple() {
    let found = messages(
        "too-many",
        "export const p: [string, number] = [\"a\", 1, true];\n",
    );
    assert_eq!(found.len(), 1);
    assert!(
        found[0].contains("[string, number, boolean]"),
        "{}",
        found[0]
    );
    assert!(found[0].contains("[string, number]"), "{}", found[0]);
}

#[test]
fn too_few_elements_is_reported() {
    assert_eq!(
        codes("too-few", "export const p: [string, number] = [\"a\"];\n"),
        ["Some(2322)"]
    );
}

#[test]
fn an_unannotated_array_literal_stays_an_array() {
    // No tuple in the context: `[1, "a"]` is `(string | number)[]`, as in tsc, so it is not
    // assignable to a tuple.
    assert_eq!(
        codes(
            "stays-array",
            "const xs = [1, \"a\"];\nexport const p: [number, string] = xs;\n"
        ),
        ["Some(2322)"]
    );
}

// ---- optional and rest elements -------------------------------------------------------

#[test]
fn an_optional_element_may_be_left_out() {
    assert!(
        codes(
            "optional-ok",
            "export const a: [string, number?] = [\"a\"];\nexport const b: [string, number?] = [\"a\", 1];\n"
        )
        .is_empty()
    );
}

#[test]
fn a_tuple_with_an_optional_element_is_not_a_tuple_that_requires_it() {
    assert_eq!(
        codes(
            "optional-vs-required",
            "export function f(a: [string, number?]) {\n  const b: [string, number] = a;\n  return b;\n}\n"
        ),
        ["Some(2322)"]
    );
}

#[test]
fn a_rest_element_takes_any_number_of_trailing_elements() {
    assert!(
        codes(
            "rest-ok",
            "export const a: [string, ...number[]] = [\"a\"];\nexport const b: [string, ...number[]] = [\"a\", 1, 2, 3];\n"
        )
        .is_empty()
    );
}

#[test]
fn a_rest_element_still_checks_the_type_of_each_extra_element() {
    assert_eq!(
        codes(
            "rest-wrong",
            "export const b: [string, ...number[]] = [\"a\", 1, \"x\"];\n"
        ),
        ["Some(2322)"]
    );
}

#[test]
fn a_closed_tuple_is_not_a_tuple_with_a_rest_element() {
    assert!(
        codes(
            "closed-to-rest",
            "export function f(a: [string, number]) {\n  const b: [string, ...number[]] = a;\n  return b;\n}\n"
        )
        .is_empty()
    );
    assert_eq!(
        codes(
            "rest-to-closed",
            "export function f(a: [string, ...number[]]) {\n  const b: [string, number] = a;\n  return b;\n}\n"
        ),
        ["Some(2322)"]
    );
}

#[test]
fn a_labeled_tuple_is_the_same_as_an_unlabeled_one() {
    assert!(
        codes(
            "labeled",
            "export const p: [name: string, age?: number] = [\"a\"];\n"
        )
        .is_empty()
    );
}

// ---- tuples and arrays ----------------------------------------------------------------

#[test]
fn a_tuple_is_an_array_of_the_union_of_its_elements() {
    assert!(
        codes(
            "tuple-to-array",
            "export function f(t: [string, number]) {\n  const xs: (string | number)[] = t;\n  return xs;\n}\n"
        )
        .is_empty()
    );
    assert_eq!(
        codes(
            "tuple-to-narrow-array",
            "export function f(t: [string, number]) {\n  const xs: string[] = t;\n  return xs;\n}\n"
        ),
        ["Some(2322)"]
    );
}

#[test]
fn an_array_is_not_a_tuple() {
    assert_eq!(
        codes(
            "array-to-tuple",
            "export function f(xs: string[]) {\n  const t: [string] = xs;\n  return t;\n}\n"
        ),
        ["Some(2322)"]
    );
}

// ---- reading elements -----------------------------------------------------------------

#[test]
fn a_literal_index_reads_that_element() {
    assert!(
        codes(
            "index-ok",
            "export function f(t: [string, number]) {\n  const s: string = t[0];\n  const n: number = t[1];\n  return [s, n];\n}\n"
        )
        .is_empty()
    );
    assert_eq!(
        codes(
            "index-wrong",
            "export function f(t: [string, number]) {\n  const n: number = t[0];\n  return n;\n}\n"
        ),
        ["Some(2322)"]
    );
}

#[test]
fn an_index_past_the_end_is_ts2493() {
    assert_eq!(
        codes(
            "index-out",
            "export function f(t: [string, number]) {\n  return t[2];\n}\n"
        ),
        ["Some(2493)"]
    );
}

#[test]
fn an_optional_element_read_includes_undefined() {
    assert_eq!(
        codes(
            "index-optional",
            "export function f(t: [string, number?]) {\n  const n: number = t[1];\n  return n;\n}\n"
        ),
        ["Some(2322)"]
    );
}

#[test]
fn length_is_the_literal_length_of_a_closed_tuple() {
    assert!(
        codes(
            "length-ok",
            "export function f(t: [string, number]) {\n  const n: 2 = t.length;\n  return n;\n}\n"
        )
        .is_empty()
    );
    assert_eq!(
        codes(
            "length-wrong",
            "export function f(t: [string, number]) {\n  const n: 3 = t.length;\n  return n;\n}\n"
        ),
        ["Some(2322)"]
    );
}

#[test]
fn a_tuple_has_the_array_methods() {
    assert!(
        codes(
            "methods",
            "export function f(t: [string, number]) {\n  return t.indexOf(\"a\") + t.length;\n}\n"
        )
        .is_empty()
    );
    assert_eq!(
        codes(
            "no-such-member",
            "export function f(t: [string, number]) {\n  return t.nothere;\n}\n"
        ),
        ["Some(2339)"]
    );
}

// ---- destructuring and loops ----------------------------------------------------------

#[test]
fn destructuring_gives_each_binding_its_own_element_type() {
    assert!(
        codes(
            "destructure-ok",
            "export function f(t: [string, number]) {\n  const [a, b] = t;\n  const s: string = a;\n  const n: number = b;\n  return [s, n];\n}\n"
        )
        .is_empty()
    );
    assert_eq!(
        codes(
            "destructure-wrong",
            "export function f(t: [string, number]) {\n  const [, b] = t;\n  const s: string = b;\n  return s;\n}\n"
        ),
        ["Some(2322)"]
    );
}

#[test]
fn destructuring_past_the_end_is_reported_unless_there_is_a_default() {
    assert_eq!(
        codes(
            "destructure-past",
            "export function f(t: [string]) {\n  const [a, b] = t;\n  return [a, b];\n}\n"
        ),
        ["Some(2493)"]
    );
    assert!(
        codes(
            "destructure-default",
            "export function f(t: [string]) {\n  const [a, b = 1] = t;\n  return [a, b];\n}\n"
        )
        .is_empty()
    );
}

#[test]
fn a_rest_binding_is_the_tuple_of_what_is_left() {
    assert!(
        codes(
            "destructure-rest",
            "export function f(t: [string, number, boolean]) {\n  const [, ...others] = t;\n  const o: [number, boolean] = others;\n  return o;\n}\n"
        )
        .is_empty()
    );
}

#[test]
fn for_of_over_a_tuple_binds_the_union_of_its_elements() {
    assert!(
        codes(
            "for-of",
            "export function f(t: [string, number]) {\n  for (const x of t) {\n    const y: string | number = x;\n    void y;\n  }\n}\n"
        )
        .is_empty()
    );
    assert_eq!(
        codes(
            "for-of-wrong",
            "export function f(t: [string, number]) {\n  for (const x of t) {\n    const y: string = x;\n    void y;\n  }\n}\n"
        ),
        ["Some(2322)"]
    );
}

// ---- returns, parameters and generics -------------------------------------------------

#[test]
fn a_function_can_return_a_tuple() {
    assert!(
        codes(
            "return-ok",
            "export function pair(): [string, number] {\n  return [\"a\", 1];\n}\n"
        )
        .is_empty()
    );
    assert_eq!(
        codes(
            "return-wrong",
            "export function pair(): [string, number] {\n  return [1, \"a\"];\n}\n"
        ),
        ["Some(2322)", "Some(2322)"]
    );
}

#[test]
fn a_tuple_argument_is_checked_against_a_tuple_parameter() {
    assert!(
        codes(
            "arg-ok",
            "function f(p: [string, number]) { return p; }\nexport const r = f([\"a\", 1]);\n"
        )
        .is_empty()
    );
    assert_eq!(
        codes(
            "arg-wrong",
            "function f(p: [string, number]) { return p; }\nexport const r = f([1, \"a\"]);\n"
        ),
        // tsc takes the literal apart: one error on each wrong element.
        ["Some(2322)", "Some(2322)"]
    );
}

#[test]
fn type_parameters_are_inferred_from_the_elements_of_a_tuple() {
    assert!(
        codes(
            "infer-ok",
            "function first<T, U>(p: [T, U]): T { return p[0]; }\nexport const s: string = first([\"a\", 1]);\n"
        )
        .is_empty()
    );
    assert_eq!(
        codes(
            "infer-wrong",
            "function first<T, U>(p: [T, U]): T { return p[0]; }\nexport const n: number = first([\"a\", 1]);\n"
        ),
        ["Some(2322)"]
    );
}

#[test]
fn a_type_parameter_is_substituted_inside_a_returned_tuple() {
    assert!(
        codes(
            "substitute-ok",
            "function swap<T, U>(p: [T, U]): [U, T] { return [p[1], p[0]]; }\nexport const r: [number, string] = swap([\"a\", 1]);\n"
        )
        .is_empty()
    );
    assert_eq!(
        codes(
            "substitute-wrong",
            "function swap<T, U>(p: [T, U]): [U, T] { return [p[1], p[0]]; }\nexport const r: [string, number] = swap([\"a\", 1]);\n"
        ),
        ["Some(2322)"]
    );
}

#[test]
fn an_array_parameter_is_inferred_from_the_elements_of_a_tuple_argument() {
    assert_eq!(
        codes(
            "array-from-tuple",
            "function head<T>(xs: T[]): T { return xs[0] as T; }\nconst t: [string, number] = [\"a\", 1];\nexport const n: number = head(t);\n"
        ),
        ["Some(2322)"]
    );
}

// ---- a tuple across files -------------------------------------------------------------

#[test]
fn a_tuple_type_is_exported_to_another_file() {
    let found = check(
        "export",
        &[
            (
                "types.ts",
                "export type Pair = [string, number?];\nexport const origin: Pair = [\"a\"];\n",
            ),
            (
                "use.ts",
                "import { Pair, origin } from \"./types\";\n\
                 export const ok: Pair = [\"b\", 2];\n\
                 export const bad: Pair = [2, \"b\"];\n\
                 export const first: string = origin[0];\n\
                 export const second: number = origin[1];\n",
            ),
        ],
    );
    let lines: Vec<(String, String)> = found.into_iter().map(|(f, c, _)| (f, c)).collect();
    // `bad` is a wrong pair twice over (both elements), and `second` may be undefined.
    assert_eq!(
        lines,
        [
            ("use.ts".to_string(), "Some(2322)".to_string()),
            ("use.ts".to_string(), "Some(2322)".to_string()),
            ("use.ts".to_string(), "Some(2322)".to_string()),
        ]
    );
}

// ---- rest parameters typed as tuples, narrowing, message text -------------------------

#[test]
fn a_tuple_rest_parameter_is_called_like_separate_parameters() {
    let source = "function f(...args: [string, number]) { return args; }\n\
                  f(\"a\", 1);\n\
                  f(\"a\");\n\
                  f(\"a\", \"b\");\n\
                  function g(...args: [string, number?]) { return args; }\n\
                  g(\"a\");\n\
                  g(\"a\", 1);\n\
                  g();\n\
                  function h(...args: [string, ...number[]]) { return args; }\n\
                  h(\"a\", 1, 2);\n\
                  h(\"a\", \"x\");\n";
    // f("a") and g() are the wrong count (2554), f("a", "b") and h("a", "x") a wrong type.
    assert_eq!(
        codes("rest-tuple-params", source),
        ["Some(2345)", "Some(2345)", "Some(2554)", "Some(2554)"]
    );
}

#[test]
fn a_tuple_is_an_object_and_is_truthy() {
    assert!(
        codes(
            "narrow-typeof",
            "export function n(t: [string] | string) {\n  if (typeof t === \"object\") {\n    const x: [string] = t;\n    return x;\n  }\n  return t;\n}\n"
        )
        .is_empty()
    );
}

#[test]
fn the_message_for_a_tuple_of_the_wrong_length_says_so() {
    let too_many = messages(
        "message-many",
        "export const p: [string, number] = [\"a\", 1, true];\n",
    );
    assert!(
        too_many[0].contains("Source has 3 element(s) but target allows only 2."),
        "{}",
        too_many[0]
    );
    let too_few = messages(
        "message-few",
        "export const p: [string, number] = [\"a\"];\n",
    );
    assert!(
        too_few[0].contains("Source has 1 element(s) but target requires 2."),
        "{}",
        too_few[0]
    );
}

// ---- names inside a tuple, and spreading one into a call ------------------------------

#[test]
fn an_unknown_name_inside_a_tuple_is_reported_once() {
    assert_eq!(
        codes(
            "unknown-name",
            "export let p: [Missing, string] | undefined;\n"
        ),
        ["Some(2304)"]
    );
    assert_eq!(
        codes(
            "unknown-name-rest",
            "export let p: [string, ...Missing[]] | undefined;\n"
        ),
        ["Some(2304)"]
    );
}

#[test]
fn spreading_a_tuple_into_a_call_is_accepted() {
    // tsc matches the elements to the parameters; this checker does not, so it must at
    // least never report an error tsc would not.
    assert!(
        codes(
            "spread-tuple",
            "function f(a: string, b: number) { return a; }\nexport function g(t: [string, number]) {\n  return f(...t);\n}\n"
        )
        .is_empty()
    );
}

// The three rules about the order of a tuple's elements are ordinary errors in tsc, reported
// at the element, and the rest of the file is still checked. The parser calls them syntax errors,
// which used to refuse the whole file with no position and no code.

#[test]
fn a_required_element_after_an_optional_one_is_ts1257() {
    assert_eq!(
        codes("order-required", "export type A = [string?, number];\n"),
        ["Some(1257)"]
    );
}

#[test]
fn a_rest_element_after_a_rest_element_is_ts1265() {
    assert_eq!(
        codes(
            "order-rest-rest",
            "export type A = [...string[], ...number[]];\n"
        ),
        ["Some(1265)"]
    );
}

#[test]
fn an_optional_element_after_a_rest_element_is_ts1266() {
    assert_eq!(
        codes(
            "order-optional-rest",
            "export type A = [...string[], number?];\n"
        ),
        ["Some(1266)"]
    );
}

#[test]
fn a_required_element_after_a_rest_element_is_allowed() {
    assert!(codes("order-trailing", "export type A = [...string[], number];\n").is_empty());
}

#[test]
fn a_bad_tuple_shape_does_not_stop_the_rest_of_the_file() {
    assert_eq!(
        codes(
            "order-continues",
            "export type A = [string?, number];\nexport const n: number = \"x\";\n"
        ),
        ["Some(1257)", "Some(2322)"]
    );
}

#[test]
fn only_the_first_misplaced_element_of_a_tuple_is_reported() {
    // tsc stops at the first broken rule in a tuple, so a second required element after the
    // optional one is not a second error.
    assert_eq!(
        codes(
            "order-first-only",
            "export type A = [a?: string, b: number, c: boolean];\n"
        ),
        ["Some(1257)"]
    );
}

#[test]
fn spreading_a_type_parameter_or_a_restless_tuple_is_not_a_rest_element() {
    // `...T` is variadic, not a rest element: two of them, or one after another spread, is
    // fine, and so is an optional element before one.
    assert!(
        codes(
            "order-variadic",
            "export type G<T extends any[], U extends any[]> = [...T, ...U];\nexport type H = [...[any], ...[any]];\nexport type I<T extends any[]> = [string?, ...T];\n"
        )
        .is_empty()
    );
}

#[test]
fn the_tuple_shape_messages_are_tscs_own() {
    assert_eq!(
        messages("order-text", "export type A = [string?, number];\n"),
        ["A required element cannot follow an optional element."]
    );
}

#[test]
fn a_negative_index_is_ts2514() {
    assert_eq!(
        codes(
            "index-negative",
            "export function f(t: [string, number]) {\n  return t[-1];\n}\n"
        ),
        ["Some(2514)"]
    );
}

#[test]
fn a_negative_index_reads_undefined() {
    assert_eq!(
        codes(
            "index-negative-undefined",
            "export function f(t: [string, number]) {\n  const n: number = t[-1];\n  return n;\n}\n"
        ),
        ["Some(2322)", "Some(2514)"]
    );
}

#[test]
fn an_index_past_the_end_reads_undefined() {
    // tsc reports the index and then reads `undefined`, so assigning it to a number is a
    // second error.
    assert_eq!(
        codes(
            "index-out-undefined",
            "export function f(t: [string, number]) {\n  const n: number = t[2];\n  return n;\n}\n"
        ),
        ["Some(2322)", "Some(2493)"]
    );
}

#[test]
fn an_index_past_the_end_is_reported_at_the_index() {
    let source = "export function f(t: [string, number]) {\n  return t[2];\n}\n";
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("tuples-index-span");
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
    let error = diagnostics
        .iter()
        .find(|d| d.severity == Severity::Error)
        .expect("one error");
    assert_eq!(&source[error.start as usize..error.end as usize], "2");
}

// The reason under a tuple mismatch is tsc's, line for line.

fn message(test: &str, source: &str) -> String {
    messages(test, source).remove(0)
}

#[test]
fn an_array_for_a_tuple_says_the_source_may_have_fewer_elements() {
    assert_eq!(
        message(
            "why-array-tuple",
            "declare const a: number[];\nexport const t: [number, number] = a;\n"
        ),
        "Type 'number[]' is not assignable to type '[number, number]'.\n  Target requires 2 element(s) but source may have fewer."
    );
}

#[test]
fn a_tuple_for_an_array_names_the_union_and_then_the_member() {
    assert_eq!(
        message(
            "why-tuple-array",
            "declare const t: [number, string];\nexport const a: number[] = t;\n"
        ),
        "Type '[number, string]' is not assignable to type 'number[]'.\n  Type 'string | number' is not assignable to type 'number'.\n    Type 'string' is not assignable to type 'number'."
    );
}

#[test]
fn a_wrong_element_in_a_rest_position_is_reported_on_the_whole_literal() {
    assert_eq!(
        message(
            "why-rest-position",
            "export const t: [number, ...string[]] = [1, \"a\", 2];\n"
        ),
        "Type '[number, string, number]' is not assignable to type '[number, ...string[]]'.\n  Type at positions 1 through 2 in source is not compatible with type at position 1 in target.\n    Type 'number' is not assignable to type 'string'."
    );
}

#[test]
fn a_single_element_in_a_rest_position_names_one_position() {
    assert_eq!(
        message(
            "why-rest-one",
            "export const t: [number, ...string[]] = [1, 2];\n"
        ),
        "Type '[number, number]' is not assignable to type '[number, ...string[]]'.\n  Type at position 1 in source is not compatible with type at position 1 in target.\n    Type 'number' is not assignable to type 'string'."
    );
}

#[test]
fn undefined_in_an_optional_position_is_reported_on_the_whole_literal() {
    // With exactOptionalPropertyTypes an optional element does not accept an explicit
    // undefined, and tsc does not take the literal apart there.
    assert_eq!(
        message(
            "why-optional",
            "export const t: [number, string?] = [1, undefined];\n"
        ),
        "Type '[number, undefined]' is not assignable to type '[number, string?]'.\n  Type at position 1 in source is not compatible with type at position 1 in target."
            .to_string()
            + "\n    Type 'undefined' is not assignable to type 'string'."
    );
}

#[test]
fn a_wrong_fixed_element_is_still_reported_on_the_element() {
    assert_eq!(
        message("why-fixed", "export const t: [string, number] = [1, 2];\n"),
        "Type 'number' is not assignable to type 'string'."
    );
}
