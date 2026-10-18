use tsox::{DiagnosticCode, TypeChecker};

// Patterns that appear in almost every TypeScript file, each of which was once reported as
// an error although tsc accepts it: an empty array literal where an array type is expected,
// constructor parameter properties, a boolean discriminant tested by truthiness, and a
// generic function calling its own function-typed parameter. Each fixture is valid
// TypeScript, so any diagnostic is a false positive.

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!("fixtures/everyday-patterns/", $name))
    };
}

fn check(source: &str, file_name: &str) -> Vec<tsox::Diagnostic> {
    let result = TypeChecker::new().check_source(source, file_name);
    assert!(result.is_ok(), "fixture should at least parse: {result:?}");
    result
        .map(|checked| checked.diagnostics)
        .unwrap_or_default()
}

fn assert_clean(file_name: &str, source: &str) {
    let diagnostics = check(source, file_name);
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

fn assert_one_error(file_name: &str, source: &str, code: DiagnosticCode) {
    let diagnostics = check(source, file_name);
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert_eq!(diagnostics[0].code, code);
}

#[test]
fn empty_array_assigned_to_a_typed_variable() {
    assert_clean(
        "empty_array_assigned_to_a_typed_variable.ts",
        fixture!("empty_array_assigned_to_a_typed_variable.ts"),
    );
}

#[test]
fn empty_array_returned_from_a_function() {
    assert_clean(
        "empty_array_returned_from_a_function.ts",
        fixture!("empty_array_returned_from_a_function.ts"),
    );
}

#[test]
fn empty_array_passed_as_an_argument() {
    assert_clean(
        "empty_array_passed_as_an_argument.ts",
        fixture!("empty_array_passed_as_an_argument.ts"),
    );
}

#[test]
fn empty_array_inside_an_object_literal() {
    assert_clean(
        "empty_array_inside_an_object_literal.ts",
        fixture!("empty_array_inside_an_object_literal.ts"),
    );
}

#[test]
fn unannotated_empty_array_can_be_assigned_on() {
    assert_clean(
        "unannotated_empty_array_can_be_assigned_on.ts",
        fixture!("unannotated_empty_array_can_be_assigned_on.ts"),
    );
}

#[test]
fn class_parameter_properties_become_instance_properties() {
    assert_clean(
        "class_parameter_properties_become_instance_properties.ts",
        fixture!("class_parameter_properties_become_instance_properties.ts"),
    );
}

#[test]
fn class_parameter_property_with_a_generic_type() {
    assert_clean(
        "class_parameter_property_with_a_generic_type.ts",
        fixture!("class_parameter_property_with_a_generic_type.ts"),
    );
}

#[test]
fn boolean_discriminant_narrows_by_truthiness() {
    assert_clean(
        "boolean_discriminant_narrows_by_truthiness.ts",
        fixture!("boolean_discriminant_narrows_by_truthiness.ts"),
    );
}

#[test]
fn generic_result_type_narrows_by_truthiness() {
    assert_clean(
        "generic_result_type_narrows_by_truthiness.ts",
        fixture!("generic_result_type_narrows_by_truthiness.ts"),
    );
}

#[test]
fn generic_function_calls_its_own_callback_parameter() {
    assert_clean(
        "generic_function_calls_its_own_callback_parameter.ts",
        fixture!("generic_function_calls_its_own_callback_parameter.ts"),
    );
}

#[test]
fn generic_function_calling_itself_still_infers() {
    assert_clean(
        "generic_function_calling_itself_still_infers.ts",
        fixture!("generic_function_calling_itself_still_infers.ts"),
    );
}

#[test]
fn empty_array_does_not_hide_a_real_mismatch() {
    assert_one_error(
        "empty_array_does_not_hide_a_real_mismatch.ts",
        fixture!("empty_array_does_not_hide_a_real_mismatch.ts"),
        DiagnosticCode::DeclaredTypeMismatch,
    );
}

#[test]
fn parameter_property_is_checked_like_any_property() {
    assert_one_error(
        "parameter_property_is_checked_like_any_property.ts",
        fixture!("parameter_property_is_checked_like_any_property.ts"),
        DiagnosticCode::PropertyDoesNotExist,
    );
}

#[test]
fn boolean_discriminant_does_not_narrow_the_wrong_branch() {
    assert_one_error(
        "boolean_discriminant_does_not_narrow_the_wrong_branch.ts",
        fixture!("boolean_discriminant_does_not_narrow_the_wrong_branch.ts"),
        DiagnosticCode::PropertyDoesNotExist,
    );
}
