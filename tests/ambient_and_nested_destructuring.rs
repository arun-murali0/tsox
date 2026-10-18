use tsox::{FileId, Severity, UnitContext, Worker};

// Three diagnostics that fired where tsc is silent, found by running part-03 of the
// TypeScript corpus through scripts/compare-local.sh:
//   TS2564  a class inside `declare namespace` / `declare module "x"` is ambient
//   TS7006  a parameter of a private member of an ambient class is not asked for a type
//   TS2488  an inner array pattern of a destructured array literal (no tuple type here)

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

// ---- TS2564 ----

#[test]
fn class_in_a_declare_namespace_needs_no_initializers() {
    let found = tsc_codes(
        "declare namespace M {\n\
         \x20   export class R { source: string; global: boolean; }\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn class_in_a_nested_declare_namespace_needs_no_initializers() {
    let found = tsc_codes(
        "declare namespace A.B.Base {\n\
         \x20   export class W { id: number; }\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn class_in_a_declare_module_needs_no_initializers() {
    let found = tsc_codes(
        "declare module \"m\" {\n\
         \x20   class C { public x: number; public y: string; }\n\
         \x20   export = C;\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn class_in_a_plain_namespace_still_needs_initializers() {
    let found = tsc_codes("namespace M { export class R { source: string; } }\n");
    assert_eq!(found, vec![2564]);
}

#[test]
fn ambient_depth_is_left_after_the_block() {
    // The count must come back to zero, or every class after a declare block is skipped.
    let found = tsc_codes(
        "declare namespace M { class A { x: number; } }\n\
         class B { y: number; }\n",
    );
    assert_eq!(found, vec![2564]);
}

// ---- TS7006 ----

#[test]
fn private_methods_of_an_ambient_class_are_not_implicit_any() {
    let found = tsc_codes(
        "declare class C {\n\
         \x20   private one(context, fn);\n\
         \x20   private two(context);\n\
         \x20   private two(context, fn);\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn private_methods_of_a_class_in_a_declare_namespace_are_not_implicit_any() {
    let found = tsc_codes("declare namespace N { class C { private m(a); } }\n");
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn public_methods_of_an_ambient_class_are_still_implicit_any() {
    let found = tsc_codes("declare class C { m(a); }\n");
    assert_eq!(found, vec![7006]);
}

#[test]
fn private_methods_of_an_ordinary_class_are_still_implicit_any() {
    let found = tsc_codes("class C { private m(a) { } }\n");
    assert_eq!(found, vec![7006]);
}

// ---- TS2488 ----

#[test]
fn nested_array_patterns_of_a_literal_are_not_reported() {
    let found = tsc_codes(
        "var [x10, [y10, [z10]]] = [1, [\"hello\", [true]]];\n\
         var [a3, [b3, { x12, y12: c2 }]] = [1, [\"hello\", { x12: 5, y12: true }]];\n",
    );
    assert!(!found.contains(&2488), "{found:?}");
}

#[test]
fn nested_array_pattern_with_a_default_is_not_reported() {
    let found = tsc_codes(
        "var [a2, [b2, { x12, y12: c2 }] = [\"abc\", { x12: 10, y12: false }]] = [1, [\"hello\", { x12: 5, y12: true }]];\n",
    );
    assert!(!found.contains(&2488), "{found:?}");
}

#[test]
fn a_non_array_source_is_still_reported() {
    let found = tsc_codes("declare const u: number | undefined;\nconst [a] = u;\n");
    assert_eq!(found, vec![2488]);
}

#[test]
fn a_nested_pattern_over_only_non_arrays_is_still_reported() {
    // No element of the literal is an array, so the inner pattern has nothing to read.
    let found = tsc_codes("const [a, [b]] = [1, 2];\n");
    assert!(found.contains(&2488), "{found:?}");
}
