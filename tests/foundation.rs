use tsox::{Severity, TypeChecker};

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
}

fn check(fixture_source: &str, file_name: &str) -> Vec<tsox::Diagnostic> {
    init_tracing();
    let checker = TypeChecker::new();
    let result = checker.check_source(fixture_source, file_name);
    assert!(result.is_ok(), "fixture should at least parse: {result:?}");
    result
        .map(|checked| checked.diagnostics)
        .unwrap_or_default()
}

#[test]
fn primitive_mismatch_is_caught() {
    let source = include_str!("fixtures/foundation/primitive_mismatch.ts");
    let diagnostics = check(source, "primitive_mismatch.ts");

    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert_eq!(diagnostics[0].severity, Severity::Error);
    assert!(diagnostics[0].message.contains("not assignable"));
}

#[test]
fn correct_primitive_assignment_produces_no_diagnostics() {
    let source = include_str!("fixtures/foundation/primitive_ok.ts");
    let diagnostics = check(source, "primitive_ok.ts");

    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn function_argument_mismatch_is_caught() {
    let source = include_str!("fixtures/foundation/function_arg_mismatch.ts");
    let diagnostics = check(source, "function_arg_mismatch.ts");

    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert!(
        diagnostics[0]
            .message
            .contains("not assignable to parameter")
    );
}

#[test]
fn function_return_mismatch_is_caught() {
    let source = include_str!("fixtures/foundation/function_return_mismatch.ts");
    let diagnostics = check(source, "function_return_mismatch.ts");

    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert!(diagnostics[0].message.contains("is not assignable to type"));
}

#[test]
fn binary_operator_mismatch_is_caught() {
    let source = include_str!("fixtures/foundation/binary_op_mismatch.ts");
    let diagnostics = check(source, "binary_op_mismatch.ts");

    assert!(
        diagnostics
            .iter()
            .any(|d| d.message.contains("arithmetic operation")),
        "expected an operator-mismatch diagnostic, got: {diagnostics:?}"
    );
}

// A statement kind this checker doesn't handle must warn, not vanish. The
// fixture is a `with` statement, which still goes to push_unsupported. It used to
// be a global augmentation, then an ambient module, then a namespace, and before that a do-while, and before that a for loop whose `i = i + 1` update happened to hit
// the "unimplemented expression" warning, so the test passed only because
// assignments were unchecked -- each time the checker learned the construct the
// fixture stopped testing anything real, so it should be something with no plan to
// be supported soon.
#[test]
fn unsupported_statement_degrades_honestly_not_silently() {
    let source = include_str!("fixtures/foundation/unsupported_statement.ts");
    let diagnostics = check(source, "unsupported_statement.ts");

    assert!(
        diagnostics
            .iter()
            .any(|d| d.severity == Severity::Warning && d.message.contains("not yet checked")),
        "expected an Unsupported-style warning, got: {diagnostics:?}"
    );
}

#[test]
fn malformed_source_reports_a_parse_error_instead_of_panicking() {
    init_tracing();
    let checker = TypeChecker::new();
    let result = checker.check_source("const x = ;", "broken.ts");

    assert!(result.is_err(), "expected a parse error, got: {result:?}");
}
