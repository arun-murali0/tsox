use tsox::{Severity, TypeChecker};

fn diagnostics(fixture: &str, file_name: &str) -> Vec<tsox::Diagnostic> {
    let result = TypeChecker::new().check_source(fixture, file_name);
    assert!(result.is_ok(), "fixture should parse: {result:?}");
    result.unwrap().diagnostics
}

fn assert_clean(fixture: &str, file_name: &str) {
    let found = diagnostics(fixture, file_name);
    assert!(found.is_empty(), "expected no diagnostics, got: {found:?}");
}

#[test]
fn unused_generic_parameter_uses_its_default() {
    assert_clean(
        include_str!("fixtures/generic-callable-signatures/unused_type_parameter_default.ts"),
        "unused_type_parameter_default.ts",
    );
}

#[test]
fn explicit_unused_generic_parameter_is_preserved() {
    assert_clean(
        include_str!("fixtures/generic-callable-signatures/explicit_unused_type_parameter.ts"),
        "explicit_unused_type_parameter.ts",
    );
}

#[test]
fn generic_function_type_infers_from_arguments() {
    assert_clean(
        include_str!("fixtures/generic-callable-signatures/generic_function_type.ts"),
        "generic_function_type.ts",
    );
}

#[test]
fn generic_arrow_expression_keeps_its_own_type_parameters() {
    assert_clean(
        include_str!("fixtures/generic-callable-signatures/generic_arrow_expression.ts"),
        "generic_arrow_expression.ts",
    );
}

#[test]
fn generic_method_signature_infers_from_arguments() {
    assert_clean(
        include_str!("fixtures/generic-callable-signatures/generic_method.ts"),
        "generic_method.ts",
    );
}

#[test]
fn nested_generic_callable_scopes_do_not_leak() {
    assert_clean(
        include_str!("fixtures/generic-callable-signatures/nested_generic_callable.ts"),
        "nested_generic_callable.ts",
    );
}

#[test]
fn generic_constraint_still_rejects_invalid_inference() {
    let found = diagnostics(
        include_str!("fixtures/generic-callable-signatures/generic_constraint_violation.ts"),
        "generic_constraint_violation.ts",
    );
    assert!(
        found
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error),
        "expected a constraint diagnostic, got: {found:?}"
    );
}
