use tsox::{DiagnosticCode, TypeChecker};

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
fn optional_param_can_be_omitted_or_provided() {
    let source = include_str!("fixtures/parameters-destructuring/optional_param_can_be_omitted.ts");
    let diagnostics = check(source, "optional_param_can_be_omitted.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn optional_param_does_not_excuse_a_missing_required_arg() {
    let source = include_str!(
        "fixtures/parameters-destructuring/optional_param_does_not_excuse_missing_required_arg.ts"
    );
    let diagnostics = check(
        source,
        "optional_param_does_not_excuse_missing_required_arg.ts",
    );
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert!(
        diagnostics[0].message.contains("arguments, but got"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn rest_param_accepts_any_trailing_arg_count() {
    let source = include_str!(
        "fixtures/parameters-destructuring/rest_param_accepts_any_trailing_arg_count.ts"
    );
    let diagnostics = check(source, "rest_param_accepts_any_trailing_arg_count.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn rest_param_argument_type_mismatch_is_caught() {
    let source = include_str!(
        "fixtures/parameters-destructuring/rest_param_argument_type_mismatch_is_caught.ts"
    );
    let diagnostics = check(source, "rest_param_argument_type_mismatch_is_caught.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert!(
        diagnostics[0]
            .message
            .contains("not assignable to parameter of type"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn rest_param_identifier_is_bound_as_an_array() {
    let source = include_str!(
        "fixtures/parameters-destructuring/rest_param_identifier_is_bound_as_array.ts"
    );
    let diagnostics = check(source, "rest_param_identifier_is_bound_as_array.ts");
    // This used to assert no diagnostics, on the assumption that `values[0]`
    // on a rest param bound as `number[]` is a plain `number`. Same cause as
    // the other array-indexing fixtures fixed alongside d91f583: this checker
    // adds `| undefined` to every element access, so `values[0]` assigned
    // into a `const value: number` is a real mismatch. The rest-param
    // binding itself (the thing this test actually covers) is still
    // correct: `values` is bound as `number[]`, not `any` or something
    // unbound, which is why there's exactly this one diagnostic and not, say,
    // a property-does-not-exist error.
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic (the honest element | undefined vs number mismatch), got: {diagnostics:?}"
    );
    assert!(
        diagnostics[0].message.contains("not assignable to type"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn object_destructuring_binds_property_types_and_checks_clean() {
    let source = include_str!(
        "fixtures/parameters-destructuring/object_destructuring_binds_property_types.ts"
    );
    let diagnostics = check(source, "object_destructuring_binds_property_types.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn object_destructuring_missing_property_is_caught() {
    let source = include_str!(
        "fixtures/parameters-destructuring/object_destructuring_missing_property_is_caught.ts"
    );
    let diagnostics = check(source, "object_destructuring_missing_property_is_caught.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert!(
        diagnostics[0].message.contains("does not exist"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn destructured_function_parameter_checks_clean() {
    let source = include_str!(
        "fixtures/parameters-destructuring/destructured_function_parameter_checks_clean.ts"
    );
    let diagnostics = check(source, "destructured_function_parameter_checks_clean.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn array_destructuring_binds_element_type_but_includes_undefined() {
    let source =
        include_str!("fixtures/parameters-destructuring/array_destructuring_binds_element_type.ts");
    let diagnostics = check(source, "array_destructuring_binds_element_type.ts");
    // This used to assert no diagnostics, on the assumption that destructuring
    // `pair: number[]` gives `first`/`second` a plain `number`. Real tsc
    // disagrees (confirmed by scripts/ts-diag-tool/compare.js): destructuring
    // past the end of a real array yields undefined at runtime, the same as a
    // numeric-literal index does, so each element includes undefined and using
    // it directly in arithmetic is an error.
    // Under noUncheckedIndexedAccess (the reference config) tsc reports TS18048 on
    // each operand: `first` and `second` are both possibly undefined.
    assert_eq!(
        diagnostics.len(),
        2,
        "expected one diagnostic per operand, got: {diagnostics:?}"
    );
    assert!(
        diagnostics
            .iter()
            .all(|d| d.code == DiagnosticCode::PossiblyUndefined),
        "got: {diagnostics:?}"
    );
    assert!(
        diagnostics[0]
            .message
            .contains("'first' is possibly 'undefined'")
            && diagnostics[1]
                .message
                .contains("'second' is possibly 'undefined'"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn array_destructuring_element_type_mismatch_is_caught() {
    let source = include_str!(
        "fixtures/parameters-destructuring/array_destructuring_element_type_mismatch_is_caught.ts"
    );
    let diagnostics = check(
        source,
        "array_destructuring_element_type_mismatch_is_caught.ts",
    );
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert!(
        diagnostics[0].message.contains("not assignable"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn destructuring_default_value_checks_clean() {
    let source = include_str!(
        "fixtures/parameters-destructuring/destructuring_default_value_checks_clean.ts"
    );
    let diagnostics = check(source, "destructuring_default_value_checks_clean.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn object_destructuring_renamed_binding_checks_clean() {
    let source = include_str!(
        "fixtures/parameters-destructuring/object_destructuring_renamed_binding_checks_clean.ts"
    );
    let diagnostics = check(
        source,
        "object_destructuring_renamed_binding_checks_clean.ts",
    );
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn array_destructuring_a_non_array_source_is_caught() {
    let source = include_str!(
        "fixtures/parameters-destructuring/array_destructuring_non_array_source_is_caught.ts"
    );
    let diagnostics = check(source, "array_destructuring_non_array_source_is_caught.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert!(
        diagnostics[0].message.contains("[Symbol.iterator]()"),
        "got: {diagnostics:?}"
    );
}
