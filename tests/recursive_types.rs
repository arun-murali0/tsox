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

// Before this, a self-reference inside an interface's own body hit
// Resolution::Circular, which made the whole interface unresolvable, so
// `node.next` here would have silently typed as `error`, and this genuine
// mismatch would never have been reported at all.
// tsc: "Type 'ListNode | null' is not assignable to type 'number'."
#[test]
fn linked_list_interface_resolves() {
    let source = include_str!("fixtures/recursive-types/linked_list_interface_resolves.ts");
    let diagnostics = check(source, "linked_list_interface_resolves.ts");
    let errors = errors(&diagnostics);
    assert_eq!(errors.len(), 1, "got: {diagnostics:?}");
    assert_eq!(errors[0].code, DiagnosticCode::ReturnTypeMismatch);
}

#[test]
fn linked_list_property_mismatch_is_caught() {
    let source =
        include_str!("fixtures/recursive-types/linked_list_property_mismatch_is_caught.ts");
    let diagnostics = check(source, "linked_list_property_mismatch_is_caught.ts");
    let errors = errors(&diagnostics);
    assert_eq!(errors.len(), 1, "got: {diagnostics:?}");
    assert_eq!(errors[0].code, DiagnosticCode::DeclaredTypeMismatch);
}

// A resolves B resolves A: A's placeholder is registered before A's own body
// is resolved, so when B's body refers back to A, it finds that placeholder
// instead of failing circularly, and by the time A's body finishes, B (and
// the placeholder it captured) is already complete.
#[test]
fn mutually_recursive_interfaces_resolve() {
    let source = include_str!("fixtures/recursive-types/mutually_recursive_interfaces_resolve.ts");
    let diagnostics = check(source, "mutually_recursive_interfaces_resolve.ts");
    let errors = errors(&diagnostics);
    assert_eq!(errors.len(), 1, "got: {diagnostics:?}");
    assert_eq!(errors[0].code, DiagnosticCode::ReturnTypeMismatch);
}

#[test]
fn recursive_class_resolves() {
    let source = include_str!("fixtures/recursive-types/recursive_class_resolves.ts");
    let diagnostics = check(source, "recursive_class_resolves.ts");
    let errors = errors(&diagnostics);
    assert_eq!(errors.len(), 1, "got: {diagnostics:?}");
    assert_eq!(errors[0].code, DiagnosticCode::ReturnTypeMismatch);
}

#[test]
fn recursive_type_literal_alias_resolves() {
    let source = include_str!("fixtures/recursive-types/recursive_type_literal_alias_resolves.ts");
    let diagnostics = check(source, "recursive_type_literal_alias_resolves.ts");
    let errors = errors(&diagnostics);
    assert_eq!(errors.len(), 1, "got: {diagnostics:?}");
    assert_eq!(errors[0].code, DiagnosticCode::ReturnTypeMismatch);
}

// The common recursive-generic shape, referring to itself with its own,
// same type parameter (`next: Box<T>`), resolves correctly. A self-reference
// with a *different*, fixed type argument (e.g. a Box<T> that also carries a
// hardcoded Box<string>) is a known remaining gap: the placeholder is still
// empty at that point, so substitution can't yet tell it apart from the bare
// generic shape. Not exercised here since it doesn't work yet.
//
// This fixture's own body has no variable declaration; box.value (number)
// is returned from a function declared to return string, so the real
// mismatch here is a return-type mismatch, not a declared (variable) type
// mismatch. See chain()'s `return box.value;` in the fixture.
#[test]
fn generic_recursive_interface_with_same_type_param_resolves() {
    let source = include_str!(
        "fixtures/recursive-types/generic_recursive_interface_with_same_type_param_resolves.ts"
    );
    let diagnostics = check(
        source,
        "generic_recursive_interface_with_same_type_param_resolves.ts",
    );
    let errors = errors(&diagnostics);
    assert_eq!(errors.len(), 1, "got: {diagnostics:?}");
    assert_eq!(errors[0].code, DiagnosticCode::ReturnTypeMismatch);
}

// `box.next` is `Box<number> | null`, so `.value` read through it is a number.
// Substitution used to cut the self-reference and leave `next` pointing at the
// generic `Box<T>`, which made `rest.value` read as T; T is not assignable to
// number, so the clean function below was reported as well. The cycle now closes
// on the substituted copy, leaving only the genuine mismatch in wrongNextValue.
#[test]
fn generic_recursive_property_is_substituted_through_the_cycle() {
    let source = include_str!("fixtures/recursive-types/generic_recursive_next_is_substituted.ts");
    let diagnostics = check(source, "generic_recursive_next_is_substituted.ts");
    let errors = errors(&diagnostics);
    assert_eq!(errors.len(), 1, "got: {diagnostics:?}");
    assert_eq!(errors[0].code, DiagnosticCode::ReturnTypeMismatch);
}
