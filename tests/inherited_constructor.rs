use tsox::{FileId, Severity, UnitContext, Worker};

// A class with no constructor of its own takes the parameters of the class it extends.
// Found in part-04 of the TypeScript corpus (errorForwardReferenceForwadingConstructor,
// es6ClassTest2): `new derived(4)` was TS2554, "Expected 0 arguments".

fn tsc_codes(source: &str) -> Vec<u32> {
    let ctx = UnitContext::new(FileId::ROOT, "a.ts", source);
    Worker::new()
        .check_unit(&ctx)
        .unwrap()
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .filter_map(|d| d.code.tsc_code())
        .collect()
}

const BASE: &str = "class base { constructor(public n: number) { } }\n";

#[test]
fn derived_class_without_a_constructor_takes_the_base_arguments() {
    let found = tsc_codes(&format!(
        "{BASE}class derived extends base {{ }}\nvar d = new derived(4);\n"
    ));
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn derived_class_used_before_its_declaration_in_a_function() {
    let found = tsc_codes(
        "function f() {\n\
         \x20   var d1 = new derived();\n\
         \x20   var d2 = new derived(4);\n\
         }\n\
         class base { constructor(public n: number) { } }\n\
         class derived extends base { }\n",
    );
    // `new derived()` is the one real error here: base wants an argument.
    assert_eq!(found, vec![2554]);
}

#[test]
fn missing_argument_is_still_reported() {
    let found = tsc_codes(&format!(
        "{BASE}class derived extends base {{ }}\nvar d = new derived();\n"
    ));
    assert_eq!(found, vec![2554]);
}

#[test]
fn wrong_argument_type_is_still_reported() {
    let found = tsc_codes(&format!(
        "{BASE}class derived extends base {{ }}\nvar d = new derived(\"a\");\n"
    ));
    assert_eq!(found, vec![2345]);
}

#[test]
fn constructor_is_inherited_through_more_than_one_level() {
    let found = tsc_codes(&format!(
        "{BASE}class mid extends base {{ }}\nclass leaf extends mid {{ }}\n\
         var ok = new leaf(1);\nvar bad = new leaf();\n"
    ));
    assert_eq!(found, vec![2554]);
}

#[test]
fn own_constructor_replaces_the_inherited_one() {
    let found = tsc_codes(&format!(
        "{BASE}class derived extends base {{ constructor() {{ super(1); }} }}\n\
         var ok = new derived();\nvar bad = new derived(1);\n"
    ));
    assert_eq!(found, vec![2554]);
}

#[test]
fn class_without_a_base_or_constructor_still_takes_nothing() {
    let found = tsc_codes("class P { }\nvar p = new P(1);\n");
    assert_eq!(found, vec![2554]);
}

#[test]
fn ambient_base_constructor_is_followed() {
    let found = tsc_codes(
        "declare class ext { constructor(a: number); }\n\
         class derived extends ext { }\n\
         var ok = new derived(1);\nvar bad = new derived();\n",
    );
    assert_eq!(found, vec![2554]);
}

// A generic base is not followed (its parameters may name its own type parameters), so the
// derived class accepts any arguments rather than risk an error tsc would not give.
#[test]
fn generic_base_is_left_unchecked_not_wrongly_rejected() {
    let found = tsc_codes(
        "class box<T> { constructor(public v: T) { } }\n\
         class num extends box<number> { }\n\
         var ok = new num(1);\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn base_that_is_not_in_the_file_is_left_unchecked() {
    let found = tsc_codes(
        "declare const mixin: new (a: number) => object;\n\
         class derived extends (mixin as any) { }\n\
         var d = new derived(1);\n",
    );
    assert!(!found.contains(&2554), "{found:?}");
}
