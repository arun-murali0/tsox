use tsox::{Diagnostic, DiagnosticCode, Severity, TypeChecker};

fn check(fixture_source: &str, file_name: &str) -> Vec<Diagnostic> {
    let checker = TypeChecker::new();
    let result = checker.check_source(fixture_source, file_name);
    assert!(result.is_ok(), "fixture should at least parse: {result:?}");
    result
        .map(|checked| checked.diagnostics)
        .unwrap_or_default()
}

fn errors(diagnostics: &[Diagnostic]) -> Vec<&Diagnostic> {
    diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect()
}

fn single_error(diagnostics: &[Diagnostic], code: DiagnosticCode) -> &Diagnostic {
    let matched = errors(diagnostics);
    assert_eq!(matched.len(), 1, "got: {diagnostics:?}");
    assert_eq!(matched[0].code, code, "got: {diagnostics:?}");
    matched[0]
}

// Assignment expressions used to hit the generic "unimplemented expression
// kind" warning unconditionally; no assignability check ran at all. tsc:
// "Type 'string' is not assignable to type 'number'."
#[test]
fn variable_reassignment_type_mismatch_is_caught() {
    let source = include_str!(
        "fixtures/assignment-expressions/variable_reassignment_type_mismatch_is_caught.ts"
    );
    let diagnostics = check(source, "variable_reassignment_type_mismatch_is_caught.ts");
    single_error(&diagnostics, DiagnosticCode::DeclaredTypeMismatch);
}

#[test]
fn variable_reassignment_clean() {
    let source = include_str!("fixtures/assignment-expressions/variable_reassignment_clean.ts");
    let diagnostics = check(source, "variable_reassignment_clean.ts");
    assert!(errors(&diagnostics).is_empty(), "got: {diagnostics:?}");
}

// Assigning through a property target (box.value = ...) reuses the same
// member-access lookup a *read* of box.value already goes through.
#[test]
fn property_assignment_type_mismatch_is_caught() {
    let source = include_str!(
        "fixtures/assignment-expressions/property_assignment_type_mismatch_is_caught.ts"
    );
    let diagnostics = check(source, "property_assignment_type_mismatch_is_caught.ts");
    single_error(&diagnostics, DiagnosticCode::DeclaredTypeMismatch);
}

// `total -= "text"` is checked as the `-` operator would be, reusing
// infer_binary_expression_type, so the diagnostic here is
// BinaryOperandTypeMismatch, not DeclaredTypeMismatch. `-=` is used rather than
// `+=` on purpose: this checker's `+` deliberately accepts number + string
// (JavaScript's string concatenation, see binary.rs), so it would report
// nothing here. The wording differs from tsc's, like many others already do;
// what matters is an error fires on the right line.
#[test]
fn compound_assignment_type_mismatch_is_caught() {
    let source = include_str!(
        "fixtures/assignment-expressions/compound_assignment_type_mismatch_is_caught.ts"
    );
    let diagnostics = check(source, "compound_assignment_type_mismatch_is_caught.ts");
    single_error(&diagnostics, DiagnosticCode::ArithmeticRightOperandInvalid);
}

#[test]
fn compound_assignment_clean() {
    let source = include_str!("fixtures/assignment-expressions/compound_assignment_clean.ts");
    let diagnostics = check(source, "compound_assignment_clean.ts");
    assert!(errors(&diagnostics).is_empty(), "got: {diagnostics:?}");
}

// `x = 2` as an expression has the type of its right-hand side (number), so
// assigning that into a `string`-typed variable is the real error here, not
// the inner `x = 2`, which is itself perfectly valid. Written without parens
// because parenthesized expressions aren't unwrapped by the dispatcher yet,
// which is a separate gap from assignment checking.
#[test]
fn assignment_result_type_is_used() {
    let source = include_str!("fixtures/assignment-expressions/assignment_result_type_is_used.ts");
    let diagnostics = check(source, "assignment_result_type_is_used.ts");
    single_error(&diagnostics, DiagnosticCode::DeclaredTypeMismatch);
}
