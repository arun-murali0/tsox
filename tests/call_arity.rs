use tsox::{Diagnostic, Severity, TypeChecker};

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

// Every fixture here is expected to produce the arity error and nothing else, so
// the count also checks that naming a parameter did not add or drop a diagnostic.
fn arity_message(diagnostics: &[Diagnostic]) -> &str {
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert_eq!(diagnostics[0].severity, Severity::Error);
    &diagnostics[0].message
}

#[test]
fn a_missing_argument_is_named() {
    let source = include_str!("fixtures/call-arity/missing_argument_is_named.ts");
    let diagnostics = check(source, "missing_argument_is_named.ts");
    assert_eq!(
        arity_message(&diagnostics),
        "Expected 2 arguments, but got 1."
    );
}

#[test]
fn several_missing_arguments_are_all_named() {
    let source = include_str!("fixtures/call-arity/several_missing_arguments_are_named.ts");
    let diagnostics = check(source, "several_missing_arguments_are_named.ts");
    assert_eq!(
        arity_message(&diagnostics),
        "Expected 3 arguments, but got 1."
    );
}

#[test]
fn too_many_arguments_names_no_parameter() {
    let source = include_str!("fixtures/call-arity/too_many_arguments_names_no_parameter.ts");
    let diagnostics = check(source, "too_many_arguments_names_no_parameter.ts");
    assert_eq!(
        arity_message(&diagnostics),
        "Expected 2 arguments, but got 3."
    );
}

#[test]
fn a_destructured_parameter_drops_the_whole_list() {
    let source = include_str!("fixtures/call-arity/destructured_parameter_is_left_unnamed.ts");
    let diagnostics = check(source, "destructured_parameter_is_left_unnamed.ts");
    assert_eq!(
        arity_message(&diagnostics),
        "Expected 2 arguments, but got 0."
    );
}

#[test]
fn a_default_before_a_required_parameter_counts_as_required() {
    let source = include_str!("fixtures/call-arity/default_before_required_counts_as_required.ts");
    let diagnostics = check(source, "default_before_required_counts_as_required.ts");
    assert_eq!(
        arity_message(&diagnostics),
        "Expected 2 arguments, but got 0."
    );
}

#[test]
fn a_trailing_default_can_be_left_out() {
    let source = include_str!("fixtures/call-arity/trailing_default_is_optional.ts");
    let diagnostics = check(source, "trailing_default_is_optional.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn a_trailing_default_still_leaves_earlier_parameters_required() {
    let source =
        include_str!("fixtures/call-arity/trailing_default_still_requires_earlier_parameters.ts");
    let diagnostics = check(
        source,
        "trailing_default_still_requires_earlier_parameters.ts",
    );
    assert_eq!(
        arity_message(&diagnostics),
        "Expected 1-2 arguments, but got 0."
    );
}

#[test]
fn parameter_names_survive_generic_substitution() {
    let source = include_str!("fixtures/call-arity/generic_instantiation_keeps_parameter_names.ts");
    let diagnostics = check(source, "generic_instantiation_keeps_parameter_names.ts");
    assert_eq!(
        arity_message(&diagnostics),
        "Expected 2 arguments, but got 1."
    );
}
