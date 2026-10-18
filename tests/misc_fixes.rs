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

#[test]
fn undefined_as_a_value_is_recognized() {
    let source = include_str!("fixtures/misc-fixes/undefined_as_a_value_is_recognized.ts");
    let diagnostics = check(source, "undefined_as_a_value_is_recognized.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn any_plus_number_infers_any_not_string() {
    let source = include_str!("fixtures/misc-fixes/any_plus_number_infers_any_not_string.ts");
    let diagnostics = check(source, "any_plus_number_infers_any_not_string.ts");
    assert!(
        diagnostics.is_empty(),
        "expected `any + 1` to stay `any` (assignable to number), got: {diagnostics:?}"
    );
}

#[test]
fn any_minus_number_infers_number_not_any() {
    let source = include_str!("fixtures/misc-fixes/any_minus_number_infers_number_not_any.ts");
    let diagnostics = check(source, "any_minus_number_infers_number_not_any.ts");
    // Confirmed against real tsc, not assumed: `any - 1` genuinely is `number`,
    // unlike `any + 1`, which stays `any`. This should report the real mismatch
    // between that `number` and the `string` target, not stay silent.
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert_eq!(diagnostics[0].severity, Severity::Error);
    assert_eq!(diagnostics[0].code, DiagnosticCode::DeclaredTypeMismatch);
}

#[test]
fn loose_equality_with_null_matches_undefined_too() {
    let source =
        include_str!("fixtures/misc-fixes/loose_equality_with_null_matches_undefined_too.ts");
    let diagnostics = check(source, "loose_equality_with_null_matches_undefined_too.ts");
    assert!(
        diagnostics.is_empty(),
        "expected `== null` to narrow out both null and undefined, got: {diagnostics:?}"
    );
}

// The case the loose-equality fix must not break: `===` still only narrows the
// exact type on its right-hand side, so undefined can still reach the return
// and the declared-type mismatch must still fire.
#[test]
fn strict_equality_with_null_does_not_match_undefined() {
    let source =
        include_str!("fixtures/misc-fixes/strict_equality_with_null_does_not_match_undefined.ts");
    let diagnostics = check(
        source,
        "strict_equality_with_null_does_not_match_undefined.ts",
    );
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert_eq!(diagnostics[0].severity, Severity::Error);
    // ReturnTypeMismatch, not DeclaredTypeMismatch: the mismatch is on a
    // `return x;`, not a `const`/`let` with a declared type.
    assert_eq!(diagnostics[0].code, DiagnosticCode::ReturnTypeMismatch);
}

#[test]
fn string_length_is_a_number() {
    let source = include_str!("fixtures/misc-fixes/string_length_is_a_number.ts");
    let diagnostics = check(source, "string_length_is_a_number.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn array_length_is_a_number() {
    let source = include_str!("fixtures/misc-fixes/array_length_is_a_number.ts");
    let diagnostics = check(source, "array_length_is_a_number.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn string_literal_object_key_is_kept() {
    let source = include_str!("fixtures/misc-fixes/string_literal_object_key_is_kept.ts");
    let diagnostics = check(source, "string_literal_object_key_is_kept.ts");
    assert!(
        diagnostics.is_empty(),
        "expected the string-literal key to be kept, got: {diagnostics:?}"
    );
}

#[test]
fn numeric_object_key_is_kept() {
    let source = include_str!("fixtures/misc-fixes/numeric_object_key_is_kept.ts");
    let diagnostics = check(source, "numeric_object_key_is_kept.ts");
    assert!(
        diagnostics.is_empty(),
        "expected the numeric key to be kept under its string form, got: {diagnostics:?}"
    );
}

#[test]
fn repeated_interface_declarations_merge() {
    let source = include_str!("fixtures/misc-fixes/repeated_interface_declarations_merge.ts");
    let diagnostics = check(source, "repeated_interface_declarations_merge.ts");
    assert!(
        diagnostics.is_empty(),
        "expected both declarations' members to be visible, got: {diagnostics:?}"
    );
}
