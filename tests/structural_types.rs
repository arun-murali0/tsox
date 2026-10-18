use tsox::{DiagnosticCode, Severity, TypeChecker};

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
fn extra_property_on_a_non_fresh_object_is_allowed() {
    let source = include_str!("fixtures/structural-types/width_subtyping_extra_prop_ok.ts");
    let diagnostics = check(source, "width_subtyping_extra_prop_ok.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn missing_required_property_is_caught() {
    let source = include_str!("fixtures/structural-types/width_subtyping_missing_prop_error.ts");
    let diagnostics = check(source, "width_subtyping_missing_prop_error.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert_eq!(diagnostics[0].severity, Severity::Error);
    assert_eq!(diagnostics[0].code, DiagnosticCode::MissingProperty);
    assert!(
        diagnostics[0].message.contains(
            "Property 'y' is missing in type '{ x: number; }' but required in type 'Point'"
        )
    );
}

#[test]
fn union_assignment_checks_each_member() {
    let source = include_str!("fixtures/structural-types/union_assignment_both_directions.ts");
    let diagnostics = check(source, "union_assignment_both_directions.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert!(diagnostics[0].message.contains("not assignable"));
}

#[test]
fn array_element_type_is_covariant() {
    let source = include_str!("fixtures/structural-types/array_covariance.ts");
    let diagnostics = check(source, "array_covariance.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn simple_type_alias_resolves() {
    let source = include_str!("fixtures/structural-types/alias_resolution.ts");
    let diagnostics = check(source, "alias_resolution.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn interfaces_can_reference_each_other_regardless_of_declaration_order() {
    let source = include_str!("fixtures/structural-types/interface_forward_ref.ts");
    let diagnostics = check(source, "interface_forward_ref.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn excess_property_on_a_fresh_literal_is_an_error() {
    let source = include_str!("fixtures/structural-types/excess_property_literal.ts");
    let diagnostics = check(source, "excess_property_literal.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert_eq!(diagnostics[0].severity, Severity::Error);
    assert!(
        diagnostics[0]
            .message
            .contains("Object literal may only specify known properties"),
        "got: {diagnostics:?}"
    );
    assert!(
        diagnostics[0].message.contains("'b'"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn excess_property_in_a_nested_literal_is_an_error() {
    let source = include_str!("fixtures/structural-types/nested_excess_property_literal.ts");
    let diagnostics = check(source, "nested_excess_property_literal.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert_eq!(diagnostics[0].severity, Severity::Error);
    assert!(
        diagnostics[0].message.contains("'extra'"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn excess_property_is_caught_in_return_and_argument_positions() {
    let source =
        include_str!("fixtures/structural-types/excess_property_in_return_and_argument.ts");
    let diagnostics = check(source, "excess_property_in_return_and_argument.ts");
    assert_eq!(
        diagnostics.len(),
        2,
        "expected one diagnostic for the return and one for the argument, got: {diagnostics:?}"
    );
    assert!(diagnostics.iter().all(|d| d.severity == Severity::Error));
    assert!(diagnostics.iter().any(|d| d.message.contains("'y'")));
    assert!(diagnostics.iter().any(|d| d.message.contains("'z'")));
}

#[test]
fn function_arity_mismatches_are_caught_both_directions() {
    let source = include_str!("fixtures/structural-types/function_arity_errors.ts");
    let diagnostics = check(source, "function_arity_errors.ts");
    assert_eq!(
        diagnostics.len(),
        2,
        "expected exactly two diagnostics, got: {diagnostics:?}"
    );
    assert!(
        diagnostics
            .iter()
            .all(|d| d.message.contains("Expected 2 argument"))
    );
}

#[test]
fn missing_optional_property_is_not_an_error() {
    let source = include_str!("fixtures/structural-types/optional_property.ts");
    let diagnostics = check(source, "optional_property.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn optional_property_cannot_satisfy_required_property() {
    let source =
        include_str!("fixtures/structural-types/optional_property_cannot_satisfy_required.ts");
    let diagnostics = check(source, "optional_property_cannot_satisfy_required.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert!(diagnostics[0].message.contains("not assignable"));
}

#[test]
fn width_subtyping_with_multiple_properties() {
    let source = include_str!("fixtures/structural-types/width_subtyping_multiple_props.ts");
    let diagnostics = check(source, "width_subtyping_multiple_props.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no errors, got: {diagnostics:?}"
    );
}

#[test]
fn union_assignment_rejects_unrelated_type() {
    let source = include_str!("fixtures/structural-types/union_assignment_error.ts");
    let diagnostics = check(source, "union_assignment_error.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "expected one error, got: {diagnostics:?}"
    );
    assert!(diagnostics[0].message.contains("not assignable"));
}

#[test]
fn array_covariance_allows_subtype_assignment() {
    let source = include_str!("fixtures/structural-types/array_covariance_unsound.ts");
    let diagnostics = check(source, "array_covariance_unsound.ts");
    let type_errors: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(
        type_errors.is_empty(),
        "array covariance should not error, got: {type_errors:?}"
    );
}

// Before recursive-type resolution existed, a mutually recursive pair like
// this couldn't be resolved at all, and the checker deliberately gave up
// rather than infinite-looping, surfacing that as an "unresolved
// annotation" diagnostic was the safe, intentional behavior at the time.
// Now that mutually recursive interfaces genuinely resolve (see
// tests/recursive_types.rs's mutually_recursive_interfaces_resolve and
// friends), this fixture's code is legitimately valid TypeScript: `{} as
// any` satisfies B's `a: A` structurally, same as real tsc would accept.
// This test now asserts the improved behavior (no infinite loop, no
// diagnostics) instead of the old give-up-safely one.
#[test]
fn circular_type_reference_does_not_infinite_loop() {
    let source = include_str!("fixtures/structural-types/circular_type_reference.ts");
    let diagnostics = check(source, "circular_type_reference.ts");
    let errors: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "mutually recursive interfaces now resolve; this circular reference should no longer surface as unresolved, got: {diagnostics:?}"
    );
}

#[test]
fn nested_object_subtyping_allows_extra_properties_on_non_fresh_objects() {
    let source = include_str!("fixtures/structural-types/nested_object_subtyping.ts");
    let diagnostics = check(source, "nested_object_subtyping.ts");
    assert!(
        diagnostics.is_empty(),
        "nested width subtyping should work, got: {diagnostics:?}"
    );
}

#[test]
fn function_parameters_are_usable_inside_their_own_body() {
    let source = include_str!("fixtures/structural-types/parameter_used_in_binary_expression.ts");
    let diagnostics = check(source, "parameter_used_in_binary_expression.ts");
    assert!(
        diagnostics.is_empty(),
        "parameters should be checkable inside their own function body, got: {diagnostics:?}"
    );
}

#[test]
fn unresolvable_type_annotation_is_reported_not_silently_swallowed() {
    let source =
        include_str!("fixtures/structural-types/unresolvable_type_annotation_is_reported.ts");
    let diagnostics = check(source, "unresolvable_type_annotation_is_reported.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert_eq!(diagnostics[0].severity, Severity::Error);
    assert!(
        diagnostics[0]
            .message
            .contains("Cannot find name 'DoesNotExist'"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn ternary_of_identical_nested_object_literals_collapses_to_one_type() {
    let source = include_str!(
        "fixtures/structural-types/union_of_identical_nested_object_shapes_collapses.ts"
    );
    let diagnostics = check(
        source,
        "union_of_identical_nested_object_shapes_collapses.ts",
    );
    // The two branches are `{ inner: { count: number } }` built at different
    // sites, so their inner objects sit in different arena slots. Compared by slot
    // they stay a two-member union, and reading `.inner` off a union is an error.
    // Compared by shape they collapse into one object and the read is fine.
    assert!(
        diagnostics.is_empty(),
        "expected no diagnostics, got: {diagnostics:?}"
    );
}

#[test]
fn structurally_equal_aliases_in_a_union_narrow_to_one_usable_type() {
    let source = include_str!(
        "fixtures/structural-types/union_of_structurally_equal_aliases_narrows_cleanly.ts"
    );
    let diagnostics = check(
        source,
        "union_of_structurally_equal_aliases_narrows_cleanly.ts",
    );
    // `First | Second | null` collapses First and Second into one member, so
    // narrowing away null leaves a single object and `x.value.count` resolves.
    assert!(
        diagnostics.is_empty(),
        "expected no diagnostics, got: {diagnostics:?}"
    );
}

#[test]
fn property_declaration_order_never_changes_assignability() {
    let source =
        include_str!("fixtures/structural-types/property_declaration_order_never_matters.ts");
    let diagnostics = check(source, "property_declaration_order_never_matters.ts");
    // An interface, a class and an object literal, each declaring the same three
    // properties in a different order, are all assignable to one another.
    assert!(
        diagnostics.is_empty(),
        "expected no diagnostics, got: {diagnostics:?}"
    );
}
