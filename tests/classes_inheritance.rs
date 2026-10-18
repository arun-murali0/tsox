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
fn method_call_through_member_expression_is_checked() {
    let source =
        include_str!("fixtures/classes-inheritance/method_call_through_member_expression.ts");
    let diagnostics = check(source, "method_call_through_member_expression.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn extends_flattens_inherited_fields() {
    let source = include_str!("fixtures/classes-inheritance/extends_inherits_fields.ts");
    let diagnostics = check(source, "extends_inherits_fields.ts");
    // Both fields have no initializer, which is TS2564. That is the only thing
    // reported: the literal assigned to Dog is not flagged, which is what shows
    // Animal's `name` is flattened into Dog.
    assert_eq!(
        diagnostics.len(),
        2,
        "expected exactly the two uninitialized-property errors, got: {diagnostics:?}"
    );
    assert!(
        diagnostics
            .iter()
            .all(|d| d.severity == Severity::Error && d.message.contains("has no initializer")),
        "got: {diagnostics:?}"
    );
}

#[test]
fn missing_an_inherited_field_is_caught() {
    let source = include_str!("fixtures/classes-inheritance/extends_missing_inherited_field.ts");
    let diagnostics = check(source, "extends_missing_inherited_field.ts");
    assert_eq!(
        diagnostics.len(),
        3,
        "expected two uninitialized-property errors plus the missing-field mismatch, got: {diagnostics:?}"
    );
    let uninitialized = diagnostics
        .iter()
        .filter(|d| d.message.contains("has no initializer"))
        .count();
    assert_eq!(uninitialized, 2, "got: {diagnostics:?}");
}

#[test]
fn new_expression_checks_constructor_arity() {
    let source = include_str!("fixtures/classes-inheritance/new_expression_arity_mismatch.ts");
    let diagnostics = check(source, "new_expression_arity_mismatch.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert!(diagnostics[0].message.contains("Expected 2 argument"));
}

#[test]
fn this_expression_resolves_to_the_class_instance_type() {
    let source = include_str!("fixtures/classes-inheritance/this_expression_known_gap.ts");
    let diagnostics = check(source, "this_expression_known_gap.ts");
    // The only diagnostic is the uninitialized `count`; `this.count` itself
    // resolves correctly, so nothing is reported at the return.
    assert_eq!(
        diagnostics.len(),
        1,
        "expected `this.count` to resolve correctly, got: {diagnostics:?}"
    );
    assert!(
        diagnostics[0].message.contains("has no initializer"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn this_expression_type_mismatch_is_caught() {
    let source = include_str!("fixtures/classes-inheritance/this_expression_type_mismatch.ts");
    let diagnostics = check(source, "this_expression_type_mismatch.ts");
    assert_eq!(
        diagnostics.len(),
        2,
        "expected the uninitialized `count` and the return mismatch, got: {diagnostics:?}"
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message.contains("has no initializer")),
        "got: {diagnostics:?}"
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message.contains("is not assignable to type")),
        "got: {diagnostics:?}"
    );
}

#[test]
fn untyped_constructor_param_skips_arity_check_instead_of_false_flagging() {
    let source =
        include_str!("fixtures/classes-inheritance/untyped_constructor_param_skips_arity_check.ts");
    let diagnostics = check(source, "untyped_constructor_param_skips_arity_check.ts");
    assert_eq!(
        diagnostics.len(),
        3,
        "expected two implicit-any errors (x and y) and one skipped-arity warning, got: {diagnostics:?}"
    );

    let implicit_any: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.message.contains("implicitly has an 'any' type"))
        .collect();
    assert_eq!(implicit_any.len(), 2, "got: {diagnostics:?}");
    assert!(implicit_any.iter().all(|d| d.severity == Severity::Error));

    let skipped_arity = diagnostics
        .iter()
        .find(|d| d.message.contains("untyped parameter"));
    assert_eq!(
        skipped_arity.map(|diagnostic| diagnostic.severity),
        Some(Severity::Warning),
        "got: {diagnostics:?}"
    );
}

#[test]
fn one_unresolvable_member_makes_the_whole_class_unsupported() {
    let source = include_str!(
        "fixtures/classes-inheritance/one_unresolvable_member_makes_whole_class_unsupported.ts"
    );
    let diagnostics = check(
        source,
        "one_unresolvable_member_makes_whole_class_unsupported.ts",
    );
    assert_eq!(
        diagnostics.len(),
        3,
        "expected the implicit-any error, the uninitialized `x`, and the unsupported-class warning, got: {diagnostics:?}"
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error && d.message.contains("has no initializer")),
        "an unsupported class still reports its uninitialized fields, got: {diagnostics:?}"
    );

    let implicit_any = diagnostics
        .iter()
        .find(|d| d.message.contains("implicitly has an 'any' type"));
    assert_eq!(
        implicit_any.map(|diagnostic| diagnostic.severity),
        Some(Severity::Error),
        "got: {diagnostics:?}"
    );

    let unsupported = diagnostics.iter().find(|d| d.severity == Severity::Warning);
    assert!(
        unsupported.is_some_and(|d| d.message.contains("Point")),
        "got: {diagnostics:?}"
    );
}

#[test]
fn untyped_method_param_skips_arity_not_whole_class() {
    let source = include_str!(
        "fixtures/classes-inheritance/untyped_method_param_skips_arity_not_whole_class.ts"
    );
    let diagnostics = check(
        source,
        "untyped_method_param_skips_arity_not_whole_class.ts",
    );

    assert_eq!(
        diagnostics.len(),
        5,
        "expected exactly five diagnostics, got: {diagnostics:?}"
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error && d.message.contains("has no initializer")),
        "expected an uninitialized-property error for `count`, got: {diagnostics:?}"
    );

    let implicit_any = diagnostics
        .iter()
        .find(|d| d.message.contains("implicitly has an 'any' type"));
    assert_eq!(
        implicit_any.map(|diagnostic| diagnostic.severity),
        Some(Severity::Error),
        "expected an implicit-any error for `value`, got: {diagnostics:?}"
    );

    let count_mismatch = diagnostics
        .iter()
        .find(|d| d.message.contains("not assignable"));
    assert!(
        count_mismatch.is_some(),
        "expected a type-mismatch diagnostic for `count`, got: {diagnostics:?}"
    );
    assert_eq!(
        count_mismatch.map(|diagnostic| diagnostic.severity),
        Some(Severity::Error)
    );

    let untyped_warning = diagnostics
        .iter()
        .find(|d| d.message.contains("untyped parameter"));
    assert!(
        untyped_warning.is_some(),
        "expected an untyped-parameter warning, got: {diagnostics:?}"
    );
    assert_eq!(
        untyped_warning.map(|diagnostic| diagnostic.severity),
        Some(Severity::Warning)
    );

    let binary_op_mismatch = diagnostics
        .iter()
        .find(|d| d.message.contains("arithmetic operation"));
    assert!(
        binary_op_mismatch.is_some(),
        "expected the `1 - \"x\"` mismatch inside the call to still be caught, got: {diagnostics:?}"
    );
    assert_eq!(
        binary_op_mismatch.map(|diagnostic| diagnostic.severity),
        Some(Severity::Error)
    );
}

#[test]
fn property_initialization_is_satisfied_by_every_recognized_form() {
    let source =
        include_str!("fixtures/classes-inheritance/property_initialization_is_satisfied.ts");
    let diagnostics = check(source, "property_initialization_is_satisfied.ts");
    let errors: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "an initializer, a constructor assignment, `!`, `?`, an undefined-including or `any` type, \
         and a static property must all be accepted, got: {diagnostics:?}"
    );
}

#[test]
fn property_never_assigned_in_the_constructor_is_flagged() {
    let source = include_str!(
        "fixtures/classes-inheritance/property_missing_from_constructor_is_flagged.ts"
    );
    let diagnostics = check(source, "property_missing_from_constructor_is_flagged.ts");
    let errors: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert_eq!(
        errors.len(),
        1,
        "only `forgotten` is unassigned, got: {diagnostics:?}"
    );
    assert!(
        errors[0].message.contains("'forgotten'"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn property_assigned_only_inside_a_callback_is_flagged() {
    let source = include_str!(
        "fixtures/classes-inheritance/property_assigned_only_inside_a_callback_is_flagged.ts"
    );
    let diagnostics = check(
        source,
        "property_assigned_only_inside_a_callback_is_flagged.ts",
    );
    let errors: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert_eq!(
        errors.len(),
        1,
        "an assignment inside a nested arrow does not count, got: {diagnostics:?}"
    );
    assert!(
        errors[0].message.contains("'value'"),
        "got: {diagnostics:?}"
    );
}

fn errors_of(diagnostics: &[tsox::Diagnostic]) -> Vec<&tsox::Diagnostic> {
    diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect()
}

// This used to be a documented gap: any `this.value = ...` anywhere in the
// constructor counted as assigning it, even inside an `if`. The check now
// follows branches (see definitely_assigned_names in statements/classes.rs), so
// an assignment that only happens when `flag` is true is reported, as in tsc.
#[test]
fn conditional_constructor_assignment_is_now_caught() {
    let source = include_str!(
        "fixtures/classes-inheritance/constructor_conditional_assignment_is_now_caught.ts"
    );
    let diagnostics = check(
        source,
        "constructor_conditional_assignment_is_now_caught.ts",
    );
    let errors = errors_of(&diagnostics);
    assert_eq!(errors.len(), 1, "got: {diagnostics:?}");
    assert!(
        errors[0].message.contains("no initializer"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn assignment_in_only_one_if_else_branch_is_flagged() {
    let source = include_str!(
        "fixtures/classes-inheritance/constructor_assigned_in_only_one_branch_is_flagged.ts"
    );
    let diagnostics = check(
        source,
        "constructor_assigned_in_only_one_branch_is_flagged.ts",
    );
    assert_eq!(errors_of(&diagnostics).len(), 1, "got: {diagnostics:?}");
}

#[test]
fn assignment_in_both_if_else_branches_is_satisfied() {
    let source = include_str!(
        "fixtures/classes-inheritance/constructor_assigned_in_both_branches_is_satisfied.ts"
    );
    let diagnostics = check(
        source,
        "constructor_assigned_in_both_branches_is_satisfied.ts",
    );
    assert!(errors_of(&diagnostics).is_empty(), "got: {diagnostics:?}");
}

// `if (!flag) throw ...;` then an unconditional assignment: the throwing path
// never reaches the end of the constructor, so it does not need to assign.
#[test]
fn guard_clause_then_assignment_is_satisfied() {
    let source = include_str!(
        "fixtures/classes-inheritance/constructor_guard_clause_then_assignment_is_satisfied.ts"
    );
    let diagnostics = check(
        source,
        "constructor_guard_clause_then_assignment_is_satisfied.ts",
    );
    assert!(errors_of(&diagnostics).is_empty(), "got: {diagnostics:?}");
}
