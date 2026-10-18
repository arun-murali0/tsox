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

// Problem: a batch of gaps found by reading src/ (false errors on Record, spread,
// `any` under a null check, class accessors, and unary/template expressions that
// fell to the "not yet checked" catch-all).
// Now: each fixture is either clean (no diagnostic of any severity, so a leftover
// "not yet checked" warning also fails it) or reports exactly one error.
fn assert_clean(diagnostics: &[Diagnostic]) {
    assert!(
        diagnostics.is_empty(),
        "expected no diagnostics, got: {diagnostics:?}"
    );
}

fn the_one_error(diagnostics: &[Diagnostic]) -> &Diagnostic {
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert_eq!(diagnostics[0].severity, Severity::Error);
    &diagnostics[0]
}

#[test]
fn a_record_literal_has_no_excess_properties() {
    let source =
        include_str!("fixtures/false-positive-fixes/record_literal_has_no_excess_properties.ts");
    assert_clean(&check(source, "record_literal_has_no_excess_properties.ts"));
}

#[test]
fn a_record_value_literal_is_still_checked_for_excess() {
    let source = include_str!(
        "fixtures/false-positive-fixes/record_value_literal_is_still_checked_for_excess.ts"
    );
    let diagnostics = check(
        source,
        "record_value_literal_is_still_checked_for_excess.ts",
    );
    let error = the_one_error(&diagnostics);
    assert!(
        error.message.contains("'y' does not exist"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn a_spread_argument_is_not_counted_as_one_argument() {
    let source = include_str!(
        "fixtures/false-positive-fixes/spread_argument_is_not_counted_as_one_argument.ts"
    );
    assert_clean(&check(
        source,
        "spread_argument_is_not_counted_as_one_argument.ts",
    ));
}

// tsc's TS2556: an array spread where it covers required parameters is reported on the
// spread itself, and nothing else is said about the call.
fn assert_only_spread_errors(diagnostics: &[Diagnostic], expected: usize) {
    assert_eq!(
        diagnostics.len(),
        expected,
        "expected {expected} diagnostic(s), got: {diagnostics:?}"
    );
    for diagnostic in diagnostics {
        assert_eq!(diagnostic.severity, Severity::Error);
        assert_eq!(
            diagnostic.code,
            DiagnosticCode::SpreadArgumentNotTupleOrRest
        );
    }
}

#[test]
fn an_array_spread_must_land_on_a_rest_parameter() {
    let source =
        include_str!("fixtures/false-positive-fixes/spread_array_must_land_on_a_rest_parameter.ts");
    assert_only_spread_errors(
        &check(source, "spread_array_must_land_on_a_rest_parameter.ts"),
        2,
    );
}

#[test]
fn a_spread_after_every_parameter_is_still_reported() {
    let source = include_str!(
        "fixtures/false-positive-fixes/spread_after_every_parameter_is_still_reported.ts"
    );
    assert_only_spread_errors(
        &check(source, "spread_after_every_parameter_is_still_reported.ts"),
        1,
    );
}

#[test]
fn an_array_spread_contributes_its_element_type() {
    let source =
        include_str!("fixtures/false-positive-fixes/array_spread_contributes_its_element_type.ts");
    assert_clean(&check(
        source,
        "array_spread_contributes_its_element_type.ts",
    ));
}

#[test]
fn an_array_spread_element_type_is_not_dropped() {
    let source =
        include_str!("fixtures/false-positive-fixes/array_spread_element_type_is_not_dropped.ts");
    let diagnostics = check(source, "array_spread_element_type_is_not_dropped.ts");
    let error = the_one_error(&diagnostics);
    assert!(
        error.message.contains("not assignable"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn any_survives_a_null_check() {
    let source = include_str!("fixtures/false-positive-fixes/any_survives_a_null_check.ts");
    assert_clean(&check(source, "any_survives_a_null_check.ts"));
}

#[test]
fn a_class_getter_is_a_property() {
    let source = include_str!("fixtures/false-positive-fixes/class_getter_is_a_property.ts");
    assert_clean(&check(source, "class_getter_is_a_property.ts"));
}

#[test]
fn a_class_getter_type_is_checked() {
    let source = include_str!("fixtures/false-positive-fixes/class_getter_type_is_checked.ts");
    let diagnostics = check(source, "class_getter_type_is_checked.ts");
    let error = the_one_error(&diagnostics);
    assert!(
        error.message.contains("not assignable"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn a_setter_alone_is_a_property() {
    let source = include_str!("fixtures/false-positive-fixes/class_setter_alone_is_a_property.ts");
    assert_clean(&check(source, "class_setter_alone_is_a_property.ts"));
}

#[test]
fn unary_and_template_literals_have_types() {
    let source =
        include_str!("fixtures/false-positive-fixes/unary_and_template_literals_have_types.ts");
    assert_clean(&check(source, "unary_and_template_literals_have_types.ts"));
}

#[test]
fn a_negative_literal_is_not_a_string() {
    let source = include_str!("fixtures/false-positive-fixes/negative_literal_is_not_a_string.ts");
    let diagnostics = check(source, "negative_literal_is_not_a_string.ts");
    let error = the_one_error(&diagnostics);
    assert!(
        error.message.contains("not assignable"),
        "got: {diagnostics:?}"
    );
}

// Problem: an async function annotated Promise<T> was compared with Promise<T> itself, so
// every `return value` in it was reported as not assignable.
// Now: the body is checked against T, and a returned promise is awaited first.
#[test]
fn an_async_return_is_checked_against_the_awaited_type() {
    let source = include_str!("fixtures/false-positive-fixes/async_return_is_awaited.ts");
    assert_clean(&check(source, "async_return_is_awaited.ts"));
}

#[test]
fn an_async_return_of_the_wrong_type_still_fails() {
    let source = include_str!("fixtures/false-positive-fixes/async_return_mismatch.ts");
    let diagnostics = check(source, "async_return_mismatch.ts");
    let error = the_one_error(&diagnostics);
    assert!(
        error.message.contains("'string'") && error.message.contains("'number'"),
        "got: {diagnostics:?}"
    );
}

// Problem: `{ get x() { ... } }` gave the object a property x typed as the getter function,
// so `const n: number = obj.x` was reported as `() => any` not assignable to number.
// Now: a getter is the type it returns and a setter the type it accepts.
#[test]
fn an_object_literal_accessor_is_a_property() {
    let source =
        include_str!("fixtures/false-positive-fixes/object_literal_accessors_are_properties.ts");
    assert_clean(&check(source, "object_literal_accessors_are_properties.ts"));
}

#[test]
fn an_object_literal_getter_still_has_its_type() {
    let source = include_str!("fixtures/false-positive-fixes/object_literal_accessor_mismatch.ts");
    let diagnostics = check(source, "object_literal_accessor_mismatch.ts");
    let error = the_one_error(&diagnostics);
    assert!(
        error.message.contains("'number'") && error.message.contains("'string'"),
        "got: {diagnostics:?}"
    );
}
