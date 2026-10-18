use tsox::{Diagnostic, DiagnosticCode, Severity, TypeChecker};

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
}

fn check(fixture_source: &str, file_name: &str) -> Vec<Diagnostic> {
    init_tracing();
    let checker = TypeChecker::new();
    let result = checker.check_source(fixture_source, file_name);
    assert!(result.is_ok(), "fixture should at least parse: {result:?}");
    result
        .map(|checked| checked.diagnostics)
        .unwrap_or_default()
}

// Exactly one diagnostic, an error with the given code.
fn assert_single_error(diagnostics: &[Diagnostic], code: DiagnosticCode) {
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert_eq!(diagnostics[0].severity, Severity::Error);
    assert_eq!(diagnostics[0].code, code, "got: {diagnostics:?}");
}

#[test]
fn method_call_checks_clean() {
    let source = include_str!("fixtures/interface-methods/method_call_checks_clean.ts");
    let diagnostics = check(source, "method_call_checks_clean.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn method_argument_type_mismatch_is_caught() {
    let source =
        include_str!("fixtures/interface-methods/method_argument_type_mismatch_is_caught.ts");
    let diagnostics = check(source, "method_argument_type_mismatch_is_caught.ts");
    assert_single_error(&diagnostics, DiagnosticCode::ArgumentNotAssignable);
}

#[test]
fn method_call_arity_is_checked() {
    let source = include_str!("fixtures/interface-methods/method_call_arity_is_checked.ts");
    let diagnostics = check(source, "method_call_arity_is_checked.ts");
    assert_single_error(&diagnostics, DiagnosticCode::ArgumentArityMismatch);
}

#[test]
fn method_return_type_is_used() {
    let source = include_str!("fixtures/interface-methods/method_return_type_is_used.ts");
    let diagnostics = check(source, "method_return_type_is_used.ts");
    assert_single_error(&diagnostics, DiagnosticCode::ReturnTypeMismatch);
}

// A method declared inside a type literal goes through the same member
// resolution as one inside an interface.
#[test]
fn method_in_type_literal_is_resolved() {
    let source = include_str!("fixtures/interface-methods/method_in_type_literal_is_resolved.ts");
    let diagnostics = check(source, "method_in_type_literal_is_resolved.ts");
    assert_single_error(&diagnostics, DiagnosticCode::ArgumentNotAssignable);
}

#[test]
fn optional_method_may_be_omitted() {
    let source = include_str!("fixtures/interface-methods/optional_method_may_be_omitted.ts");
    let diagnostics = check(source, "optional_method_may_be_omitted.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn missing_required_method_is_caught() {
    let source = include_str!("fixtures/interface-methods/missing_required_method_is_caught.ts");
    let diagnostics = check(source, "missing_required_method_is_caught.ts");
    assert_single_error(&diagnostics, DiagnosticCode::MissingProperty);
}

// tsc compares method parameters bivariantly: a method taking the more specific
// type is assignable where the more general one is expected.
#[test]
fn method_parameters_are_bivariant() {
    let source = include_str!("fixtures/interface-methods/method_parameters_are_bivariant.ts");
    let diagnostics = check(source, "method_parameters_are_bivariant.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

// The same rule does not reach a function-typed property, which stays strictly
// contravariant.
#[test]
fn function_property_parameters_stay_contravariant() {
    let source = include_str!(
        "fixtures/interface-methods/function_property_parameters_stay_contravariant.ts"
    );
    let diagnostics = check(source, "function_property_parameters_stay_contravariant.ts");
    assert_single_error(&diagnostics, DiagnosticCode::ReturnTypeMismatch);
}

#[test]
fn class_method_parameters_are_bivariant() {
    let source =
        include_str!("fixtures/interface-methods/class_method_parameters_are_bivariant.ts");
    let diagnostics = check(source, "class_method_parameters_are_bivariant.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn generic_interface_method_is_substituted() {
    let source =
        include_str!("fixtures/interface-methods/generic_interface_method_is_substituted.ts");
    let diagnostics = check(source, "generic_interface_method_is_substituted.ts");
    assert_single_error(&diagnostics, DiagnosticCode::ReturnTypeMismatch);
}

#[test]
fn generic_interface_method_argument_is_checked() {
    let source =
        include_str!("fixtures/interface-methods/generic_interface_method_argument_is_checked.ts");
    let diagnostics = check(source, "generic_interface_method_argument_is_checked.ts");
    assert_single_error(&diagnostics, DiagnosticCode::ArgumentNotAssignable);
}

// `Mapper<number>` binds T only. If U were substituted away it would become
// unknown, and returning it as a string would be a false error.
#[test]
fn method_level_type_parameter_is_preserved() {
    let source =
        include_str!("fixtures/interface-methods/method_level_type_parameter_is_preserved.ts");
    let diagnostics = check(source, "method_level_type_parameter_is_preserved.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn method_level_type_parameter_result_is_used() {
    let source =
        include_str!("fixtures/interface-methods/method_level_type_parameter_result_is_used.ts");
    let diagnostics = check(source, "method_level_type_parameter_result_is_used.ts");
    assert_single_error(&diagnostics, DiagnosticCode::ReturnTypeMismatch);
}

#[test]
fn implicit_any_method_parameter_is_reported() {
    let source =
        include_str!("fixtures/interface-methods/implicit_any_method_parameter_is_reported.ts");
    let diagnostics = check(source, "implicit_any_method_parameter_is_reported.ts");
    assert_single_error(&diagnostics, DiagnosticCode::ImplicitAnyParameter);
}

// Two signatures with one name are an overload set, which has no representation
// here. The interface stays unresolved, so nothing is reported, matching tsc on
// this valid code.
#[test]
fn overloaded_method_keeps_interface_unresolved() {
    let source =
        include_str!("fixtures/interface-methods/overloaded_method_keeps_interface_unresolved.ts");
    let diagnostics = check(source, "overloaded_method_keeps_interface_unresolved.ts");
    assert!(
        diagnostics.iter().all(|d| d.severity != Severity::Error),
        "expected no errors, got: {diagnostics:?}"
    );
}
