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
    let errors = errors(diagnostics);
    assert_eq!(errors.len(), 1, "got: {diagnostics:?}");
    assert_eq!(errors[0].code, code, "got: {diagnostics:?}");
    errors[0]
}

// Array<T> was previously just an unresolvable name, since "Array" isn't
// declared anywhere a user's own namespace would find it, so a parameter
// typed Array<number> made the whole containing function unresolvable, the
// same class of problem void had. Now it resolves the same as `number[]`.
#[test]
fn array_generic_syntax_resolves_like_bracket_syntax() {
    let source = include_str!(
        "fixtures/builtin-generics/array_generic_syntax_resolves_like_bracket_syntax.ts"
    );
    let diagnostics = check(
        source,
        "array_generic_syntax_resolves_like_bracket_syntax.ts",
    );
    single_error(&diagnostics, DiagnosticCode::ReturnTypeMismatch);
}

#[test]
fn array_generic_element_type_mismatch_is_caught() {
    let source =
        include_str!("fixtures/builtin-generics/array_generic_element_type_mismatch_is_caught.ts");
    let diagnostics = check(source, "array_generic_element_type_mismatch_is_caught.ts");
    single_error(&diagnostics, DiagnosticCode::DeclaredTypeMismatch);
}

#[test]
fn readonly_array_resolves() {
    let source = include_str!("fixtures/builtin-generics/readonly_array_resolves.ts");
    let diagnostics = check(source, "readonly_array_resolves.ts");
    single_error(&diagnostics, DiagnosticCode::ReturnTypeMismatch);
}

// The element type argument is resolved through the normal namespace, so an
// Array of a user-defined interface works the same as any other element type.
#[test]
fn array_of_interface_element_resolves() {
    let source = include_str!("fixtures/builtin-generics/array_of_interface_element_resolves.ts");
    let diagnostics = check(source, "array_of_interface_element_resolves.ts");
    single_error(&diagnostics, DiagnosticCode::ReturnTypeMismatch);
}

// The practical case this was for: a Promise-returning method used to make
// the *entire* interface unresolvable (the same failure mode void had before
// it was wired up), so every other check against Logger silently stopped
// happening. tsc: "Argument of type 'number' is not assignable to parameter
// of type 'string'."
#[test]
fn promise_return_type_no_longer_blocks_resolution() {
    let source = include_str!(
        "fixtures/builtin-generics/promise_return_type_no_longer_blocks_resolution.ts"
    );
    let diagnostics = check(source, "promise_return_type_no_longer_blocks_resolution.ts");
    single_error(&diagnostics, DiagnosticCode::ArgumentNotAssignable);
}

// A Promise's then, catch and finally exist (loosely typed), and a member it does not
// have is reported once; see the fixture's own comment.
#[test]
fn promise_member_access_correctly_reports_missing_property() {
    let source = include_str!(
        "fixtures/builtin-generics/promise_member_access_correctly_reports_missing_property.ts"
    );
    let diagnostics = check(
        source,
        "promise_member_access_correctly_reports_missing_property.ts",
    );
    single_error(&diagnostics, DiagnosticCode::PropertyDoesNotExist);
}

// The practical case this was for, the same class of bug void and Promise
// both had before they were wired up: a Record-typed property used to make
// the whole containing interface unresolvable.
#[test]
fn record_no_longer_blocks_resolution() {
    let source = include_str!("fixtures/builtin-generics/record_no_longer_blocks_resolution.ts");
    let diagnostics = check(source, "record_no_longer_blocks_resolution.ts");
    single_error(&diagnostics, DiagnosticCode::DeclaredTypeMismatch);
}

#[test]
fn record_dot_access_returns_value_type() {
    let source = include_str!("fixtures/builtin-generics/record_dot_access_returns_value_type.ts");
    let diagnostics = check(source, "record_dot_access_returns_value_type.ts");
    assert!(errors(&diagnostics).is_empty(), "got: {diagnostics:?}");
}

// K is not modelled (see TypeArena::record_value_type's own doc comment),
// so this only proves a computed access resolves and returns V, not that a
// key outside K would be rejected (it wouldn't be, which is the documented,
// deliberate under-check).
#[test]
fn record_bracket_access_returns_value_type() {
    let source =
        include_str!("fixtures/builtin-generics/record_bracket_access_returns_value_type.ts");
    let diagnostics = check(source, "record_bracket_access_returns_value_type.ts");
    assert!(errors(&diagnostics).is_empty(), "got: {diagnostics:?}");
}

// Promise<T> and Record<K, V> read as an empty body, so a call such as unwrap(load())
// bound nothing and returned the bare T. The arguments are now matched off the
// applications.
#[test]
fn promise_argument_infers_the_type_parameter() {
    let source =
        include_str!("fixtures/builtin-generics/promise_argument_infers_the_type_parameter.ts");
    let diagnostics = check(source, "promise_argument_infers_the_type_parameter.ts");
    assert!(errors(&diagnostics).is_empty(), "got: {diagnostics:?}");
}

#[test]
fn promise_argument_inference_still_rejects_a_mismatch() {
    let source = include_str!(
        "fixtures/builtin-generics/promise_argument_inference_still_rejects_a_mismatch.ts"
    );
    let diagnostics = check(
        source,
        "promise_argument_inference_still_rejects_a_mismatch.ts",
    );
    single_error(&diagnostics, DiagnosticCode::DeclaredTypeMismatch);
}

#[test]
fn record_argument_infers_the_value_type() {
    let source = include_str!("fixtures/builtin-generics/record_argument_infers_the_value_type.ts");
    let diagnostics = check(source, "record_argument_infers_the_value_type.ts");
    assert!(errors(&diagnostics).is_empty(), "got: {diagnostics:?}");
}
