use tsox::{FileId, Severity, UnitContext, Worker};

// A generic function is instantiated against the signature it is compared with before
// the two are related, as tsc does, so `<T>(x: T) => T` fits `(x: number) => number`.

fn errors(source: &str) -> Vec<(Option<u32>, String)> {
    let ctx = UnitContext::new(FileId::ROOT, "a.ts", source);
    Worker::new()
        .check_unit(&ctx)
        .unwrap()
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| (d.code.tsc_code(), d.message.to_string()))
        .collect()
}

#[test]
fn generic_identity_is_accepted_as_an_argument_for_a_concrete_signature() {
    let found = errors(
        "declare function id<T>(x: T): T;\n\
         declare function apply(f: (x: number) => number): number;\n\
         apply(id);\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn generic_function_is_accepted_when_assigned_to_a_concrete_annotation() {
    let found = errors(
        "declare function id<T>(x: T): T;\n\
         const g: (x: number) => number = id;\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn value_returning_function_is_accepted_for_void_callback() {
    let found = errors(
        "declare function returnsNumber(): number;\n\
         declare function consume(callback: () => void): void;\n\
         consume(returnsNumber);\n\
         const callback: () => void = () => 123;\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn instantiation_still_rejects_a_mismatched_return_type() {
    let found = errors(
        "declare function id<T>(x: T): T;\n\
         const bad: (x: number) => string = id;\n",
    );
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn a_constraint_that_the_target_violates_is_still_reported() {
    let found = errors(
        "declare function len<T extends string>(x: T): number;\n\
         const f: (x: number) => number = len;\n",
    );
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn parameter_unused_in_the_signature_does_not_block_assignment() {
    let found = errors(
        "declare function pick<T>(x: number): number;\n\
         const f: (x: number) => number = pick;\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_target_parameter_of_any_instantiates_the_source_with_any() {
    let found = errors(
        "declare function first<T>(ts: T[]): T;\n\
         declare function run<R>(f: (c: any) => R): R;\n\
         const r: number = run<number>(first);\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

// A parameter with a default takes its type from the default, and a setter takes its
// parameter type from the getter next to it, so neither is an implicit any (TS7006). A bare
// parameter and a setter with no getter still are.
#[test]
fn defaulted_parameter_and_getter_typed_setter_are_not_implicit_any() {
    let found = errors(
        "function a(y = false) { return y; }\n\
         const f = (z = 1) => z;\n\
         class C { get p() { return 'x'; } set p(v) { } }\n",
    );
    assert!(
        found.iter().all(|(code, _)| *code != Some(7006)),
        "{found:?}"
    );
}

#[test]
fn bare_parameter_and_lone_setter_are_still_implicit_any() {
    let found = errors("function a(y) { return y; }\nclass C { set p(v) { } }\n");
    assert_eq!(
        found.iter().filter(|(code, _)| *code == Some(7006)).count(),
        2,
        "{found:?}"
    );
}

#[test]
fn unrelated_sibling_candidates_keep_the_first_and_report_the_second_argument() {
    let found = errors(
        "interface Animal { x: number }\n\
         interface Giraffe extends Animal { neck: number }\n\
         interface Elephant extends Animal { trunk: number }\n\
         function choose<T extends Animal>(left: T, right: T): T { return left; }\n\
         declare const giraffe: Giraffe;\n\
         declare const elephant: Elephant;\n\
         const animal: Animal = choose(giraffe, elephant);\n",
    );
    // tsc infers T = Giraffe (the leftmost candidate) and reports TS2345 on `elephant`.
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, Some(2345), "{found:?}");
}

#[test]
fn generic_call_contextually_types_callback_from_earlier_argument_inference() {
    let found = errors(
        "declare function use<T, U>(value: T, callback: (item: T) => U): U;\n\
         const result: number = use(1, item => item.toFixed().length);\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn explicit_callback_parameter_annotation_overrides_contextual_type() {
    let found = errors(
        "declare function use<T, U>(value: T, callback: (item: T) => U): U;\n\
         use(1, (item: string) => item.length);\n",
    );
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn generic_object_argument_contextually_types_nested_callback_after_anchor_inference() {
    let found = errors(
        "declare function use<T, U>(value: T, options: { callback: (item: T) => U }): U;\n\
         const result: number = use(1, { callback: item => item.toFixed().length });\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn explicit_nested_callback_annotation_remains_authoritative() {
    let found = errors(
        "declare function use<T, U>(value: T, options: { callback: (item: T) => U }): U;\n\
         use(1, { callback: (item: string) => item.length });\n",
    );
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn object_only_generic_argument_still_contributes_inference_candidates() {
    let found = errors(
        "declare function wrap<T>(options: { value: T }): T;\n\
         const value: number = wrap({ value: 1 });\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn explicit_type_argument_lists_must_supply_all_required_parameters() {
    let found = errors(
        "declare function pair<T, U>(left: T, right: U): U;\n\
         pair<string>('x', 1);\n",
    );
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, Some(2558), "{found:?}");
    assert_eq!(found[0].1, "Expected 2 type arguments, but got 1.");
}

#[test]
fn explicit_type_argument_lists_reject_extra_arguments() {
    let found = errors(
        "declare function once<T>(value: T): T;\n\
         once<string, number>('x');\n",
    );
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, Some(2558), "{found:?}");
    assert_eq!(found[0].1, "Expected 1 type arguments, but got 2.");
}

#[test]
fn trailing_defaulted_type_parameters_may_be_omitted_explicitly() {
    let found = errors(
        "declare function withDefault<T, U = string>(value: T, other?: U): U;\n\
         const value: string = withDefault(1);\n\
         const defaulted: string = withDefault<number>(1);\n\
         const other: number = withDefault<number, number>(1, 2);\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn block_arrow_return_type_contributes_to_generic_inference() {
    let found = errors(
        "declare function use<T, U>(value: T, callback: (item: T) => U): U;\n\
         const wrong: string = use(1, item => { return item.toFixed().length; });\n",
    );
    assert_eq!(
        found.len(),
        1,
        "a block callback returns number, not any: {found:?}"
    );
    assert_eq!(found[0].0, Some(2322), "{found:?}");
}

#[test]
fn block_function_expression_infers_return_type() {
    let found = errors(
        "const callback = function (value: number) { return value.toFixed(); };\n\
         const wrong: number = callback(1);\n",
    );
    assert_eq!(
        found.len(),
        1,
        "the function expression returns string: {found:?}"
    );
    assert_eq!(found[0].0, Some(2322), "{found:?}");
}

#[test]
fn async_block_arrow_infers_promise_of_return_type_for_generic_calls() {
    let found = errors(
        "declare function use<T, U>(value: T, callback: (item: T) => U): U;\n\
         const wrong: Promise<string> = use(1, async item => { return item.toFixed().length; });\n",
    );
    assert_eq!(
        found.len(),
        1,
        "the callback returns Promise<number>: {found:?}"
    );
    assert_eq!(found[0].0, Some(2322), "{found:?}");
}

#[test]
fn async_expression_arrow_infers_promise_return_type() {
    let found =
        errors("const wrong: Promise<string> = async (value: number) => value.toFixed().length;\n");
    assert_eq!(
        found.len(),
        1,
        "an async arrow returns Promise<number>: {found:?}"
    );
    assert_eq!(found[0].0, Some(2322), "{found:?}");
}
