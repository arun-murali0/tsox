use tsox::{DiagnosticCode, TypeChecker};

// Inference under a bound, and explicit type arguments that cannot be resolved. Each
// fixture is valid TypeScript unless its file notes one deliberate error or gap, so any
// other diagnostic is a false positive.

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!("fixtures/generics-bounds/", $name))
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
fn literal_bound_keeps_the_literal() {
    assert_clean(
        "literal_bound_keeps_the_literal.ts",
        fixture!("literal_bound_keeps_the_literal.ts"),
    );
}

#[test]
fn number_bound_keeps_the_literal() {
    assert_clean(
        "number_bound_keeps_the_literal.ts",
        fixture!("number_bound_keeps_the_literal.ts"),
    );
}

#[test]
fn literals_of_one_primitive_combine_into_a_union() {
    assert_clean(
        "literals_of_one_primitive_combine_into_a_union.ts",
        fixture!("literals_of_one_primitive_combine_into_a_union.ts"),
    );
}

#[test]
fn unbounded_parameter_still_widens_the_literal() {
    assert_clean(
        "unbounded_parameter_still_widens_the_literal.ts",
        fixture!("unbounded_parameter_still_widens_the_literal.ts"),
    );
}

#[test]
fn bound_without_primitives_still_widens() {
    assert_clean(
        "bound_without_primitives_still_widens.ts",
        fixture!("bound_without_primitives_still_widens.ts"),
    );
}

#[test]
fn unresolved_explicit_type_argument_keeps_the_others_in_place() {
    // Exactly the unknown-name error tsc gives for `Nope`, and no other.
    assert_one_error(
        "unresolved_explicit_type_argument_keeps_the_others_in_place.ts",
        fixture!("unresolved_explicit_type_argument_keeps_the_others_in_place.ts"),
        DiagnosticCode::UnresolvedIdentifier,
    );
}

#[test]
fn literals_of_different_primitives_are_not_combined() {
    assert_one_error(
        "literals_of_different_primitives_are_not_combined.ts",
        fixture!("literals_of_different_primitives_are_not_combined.ts"),
        DiagnosticCode::ArgumentNotAssignable,
    );
}
