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

#[test]
fn void_return_with_bare_return_is_clean() {
    let source = include_str!("fixtures/void-and-never/void_return_with_bare_return_is_clean.ts");
    let diagnostics = check(source, "void_return_with_bare_return_is_clean.ts");
    assert!(
        errors(&diagnostics).is_empty(),
        "expected no errors, got: {diagnostics:?}"
    );
}

// tsc: "Type 'number' is not assignable to type 'void'."
#[test]
fn void_function_returning_a_value_is_caught() {
    let source =
        include_str!("fixtures/void-and-never/void_function_returning_a_value_is_caught.ts");
    let diagnostics = check(source, "void_function_returning_a_value_is_caught.ts");
    let errors = errors(&diagnostics);
    assert_eq!(errors.len(), 1, "got: {diagnostics:?}");
    assert_eq!(errors[0].code, DiagnosticCode::ReturnTypeMismatch);
    assert!(errors[0].message.contains("'void'"), "got: {errors:?}");
}

// Before never was wired to a keyword, `: never` made resolve_ts_type return
// None, which made the whole function unresolvable, so assigning its result
// to a number (valid: never is a subtype of everything) never even got checked.
#[test]
fn never_typed_function_no_longer_blocks_resolution() {
    let source =
        include_str!("fixtures/void-and-never/never_typed_function_no_longer_blocks_resolution.ts");
    let diagnostics = check(
        source,
        "never_typed_function_no_longer_blocks_resolution.ts",
    );
    assert!(
        errors(&diagnostics).is_empty(),
        "expected no errors, got: {diagnostics:?}"
    );
}

// The practical case this was for: a void-returning method used to make the
// *entire* interface unresolvable (resolve_method_signature's `?` on the
// return type propagated out of resolve_object_members), so every other
// method on Logger, and every check against it, silently stopped happening.
// tsc: "Argument of type 'number' is not assignable to parameter of type
// 'string'."
#[test]
fn void_returning_interface_method_now_resolves() {
    let source =
        include_str!("fixtures/void-and-never/void_returning_interface_method_now_resolves.ts");
    let diagnostics = check(source, "void_returning_interface_method_now_resolves.ts");
    let errors = errors(&diagnostics);
    assert_eq!(errors.len(), 1, "got: {diagnostics:?}");
    assert_eq!(errors[0].code, DiagnosticCode::ArgumentNotAssignable);
}
