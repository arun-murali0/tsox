use tsox::{DiagnosticCode, Severity, TypeChecker};

// The rest of the intersection work (LLD 1.13), each fixture compared with the exact code and
// text tsc prints and also run against real tsc by scripts/ts-diag-tool/compare.js:
// - a call through an intersection of functions that no member accepts: TS2769 with one branch
//   per overload, TS2554 with a combined range when every member fails on the argument count,
//   and the ordinary TS2345 when only one member gets past the count;
// - a distribution into too many members: TS2590;
// - three edge cases, `T & {}`, enums and `void & primitive`.

fn errors(source: &str, file_name: &str) -> Vec<(DiagnosticCode, String)> {
    let result = TypeChecker::new().check_source(source, file_name);
    assert!(result.is_ok(), "fixture should at least parse: {result:?}");
    result
        .map(|checked| checked.diagnostics)
        .unwrap_or_default()
        .into_iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .map(|diagnostic| (diagnostic.code, diagnostic.message))
        .collect()
}

macro_rules! calls {
    ($name:literal) => {
        include_str!(concat!("fixtures/intersection-calls/", $name, ".ts"))
    };
}
macro_rules! edges {
    ($name:literal) => {
        include_str!(concat!("fixtures/intersection-edges/", $name, ".ts"))
    };
}

fn no_overload(branches: &[(&str, &str, &str)]) -> (DiagnosticCode, String) {
    let mut text = String::from("No overload matches this call.");
    for (index, (signature, argument, parameter)) in branches.iter().enumerate() {
        text.push_str(&format!(
            "\n  Overload {} of {}, '{}', gave the following error.\n    Argument of type '{}' is not assignable to parameter of type '{}'.",
            index + 1,
            branches.len(),
            signature,
            argument,
            parameter
        ));
    }
    (DiagnosticCode::NoOverloadMatches, text)
}

#[test]
fn no_overload_accepts_the_argument() {
    assert_eq!(
        errors(
            calls!("no_overload_accepts_the_argument"),
            "no_overload_accepts_the_argument.ts"
        ),
        vec![no_overload(&[
            ("(x: string): string", "boolean", "string"),
            ("(x: number): number", "boolean", "number"),
        ])]
    );
}

#[test]
fn a_property_that_is_an_overload_set_is_reported_the_same_way() {
    assert_eq!(
        errors(
            calls!("method_property_is_an_overload_set"),
            "method_property_is_an_overload_set.ts"
        ),
        vec![no_overload(&[
            ("(x: string): string", "boolean", "string"),
            ("(x: number): number", "boolean", "number"),
        ])]
    );
}

#[test]
fn three_overloads_are_numbered_among_themselves() {
    assert_eq!(
        errors(
            calls!("three_overloads_all_fail_on_one_argument"),
            "three_overloads_all_fail_on_one_argument.ts"
        ),
        vec![no_overload(&[
            ("(x: string): string", "{}", "string"),
            ("(x: number): number", "{}", "number"),
            ("(x: boolean): boolean", "{}", "boolean"),
        ])]
    );
}

#[test]
fn overloads_that_fail_on_different_arguments_are_still_one_error() {
    assert_eq!(
        errors(
            calls!("overloads_fail_on_different_arguments"),
            "overloads_fail_on_different_arguments.ts"
        ),
        vec![no_overload(&[
            ("(a: string, b: number): string", "number", "string"),
            ("(a: number, b: string): number", "number", "string"),
        ])]
    );
}

#[test]
fn every_overload_failing_the_argument_count_is_one_range() {
    assert_eq!(
        errors(
            calls!("every_overload_fails_the_argument_count"),
            "every_overload_fails_the_argument_count.ts"
        ),
        vec![(
            DiagnosticCode::ArgumentArityMismatch,
            "Expected 1-2 arguments, but got 0.".to_string()
        )]
    );
}

#[test]
fn a_single_overload_past_the_count_check_gets_its_own_error() {
    assert_eq!(
        errors(
            calls!("one_overload_passes_the_arity_check"),
            "one_overload_passes_the_arity_check.ts"
        ),
        vec![(
            DiagnosticCode::ArgumentNotAssignable,
            "Argument of type 'boolean' is not assignable to parameter of type 'string'."
                .to_string()
        )]
    );
}

#[test]
fn the_first_overload_that_accepts_the_call_is_used() {
    assert_eq!(
        errors(
            calls!("first_accepting_overload_is_used"),
            "first_accepting_overload_is_used.ts"
        ),
        vec![]
    );
}

#[test]
fn a_distribution_into_too_many_members_is_too_complex() {
    assert_eq!(
        errors(
            edges!("distribution_too_complex"),
            "distribution_too_complex.ts"
        ),
        vec![(
            DiagnosticCode::ExpressionTooComplex,
            "Expression produces a union type that is too complex to represent.".to_string()
        )]
    );
}

#[test]
fn null_is_not_assignable_to_a_type_and_empty_object() {
    assert_eq!(
        errors(
            edges!("t_and_empty_object_rejects_null"),
            "t_and_empty_object_rejects_null.ts"
        ),
        vec![(
            DiagnosticCode::DeclaredTypeMismatch,
            "Type 'null' is not assignable to type 'string & {}'.\n  Type 'null' is not assignable to type 'string'."
                .to_string()
        )]
    );
}

#[test]
fn a_literal_assigned_to_a_reduced_never_is_shown_as_written() {
    assert_eq!(
        errors(
            edges!("void_and_a_primitive_is_never"),
            "void_and_a_primitive_is_never.ts"
        ),
        vec![(
            DiagnosticCode::DeclaredTypeMismatch,
            "Type '\"a\"' is not assignable to type 'never'.".to_string()
        )]
    );
}

#[test]
fn an_enum_intersected_with_number_is_the_enum_and_with_string_is_never() {
    assert_eq!(
        errors(
            edges!("enum_intersected_with_number_is_the_enum"),
            "enum_intersected_with_number_is_the_enum.ts"
        ),
        vec![]
    );
    assert_eq!(
        errors(
            edges!("enum_intersected_with_string_is_never"),
            "enum_intersected_with_string_is_never.ts"
        ),
        vec![]
    );
}
