use tsox::{DiagnosticCode, TypeChecker};

// Fixtures for narrowing that depends on control flow rather than a single guard:
// switch cases, && and || conditions, loop tests and the joins where paths meet.
// Each fixture is valid TypeScript unless its file notes one deliberate error, so
// any other diagnostic is a false positive.

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!("fixtures/narrowing-advanced/", $name))
    };
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
}

fn check(source: &str, file_name: &str) -> Vec<tsox::Diagnostic> {
    init_tracing();
    let checker = TypeChecker::new();
    let result = checker.check_source(source, file_name);
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
fn switch_grouped_case_labels_narrow_to_the_union() {
    assert_clean(
        "switch_grouped_case_labels_narrow_to_the_union.ts",
        fixture!("switch_grouped_case_labels_narrow_to_the_union.ts"),
    );
}

#[test]
fn switch_default_narrows_to_the_remaining_literals() {
    assert_clean(
        "switch_default_narrows_to_the_remaining_literals.ts",
        fixture!("switch_default_narrows_to_the_remaining_literals.ts"),
    );
}

#[test]
fn switch_on_plain_string_narrows_to_the_literal() {
    assert_clean(
        "switch_on_plain_string_narrows_to_the_literal.ts",
        fixture!("switch_on_plain_string_narrows_to_the_literal.ts"),
    );
}

#[test]
fn switch_property_discriminant_narrows_each_case() {
    assert_clean(
        "switch_property_discriminant_narrows_each_case.ts",
        fixture!("switch_property_discriminant_narrows_each_case.ts"),
    );
}

#[test]
fn switch_property_discriminant_default_gets_the_rest() {
    assert_clean(
        "switch_property_discriminant_default_gets_the_rest.ts",
        fixture!("switch_property_discriminant_default_gets_the_rest.ts"),
    );
}

#[test]
fn if_property_discriminant_narrows_both_branches() {
    assert_clean(
        "if_property_discriminant_narrows_both_branches.ts",
        fixture!("if_property_discriminant_narrows_both_branches.ts"),
    );
}

#[test]
fn switch_typeof_narrows_each_case() {
    assert_clean(
        "switch_typeof_narrows_each_case.ts",
        fixture!("switch_typeof_narrows_each_case.ts"),
    );
}

#[test]
fn and_condition_narrows_both_operands() {
    assert_clean(
        "and_condition_narrows_both_operands.ts",
        fixture!("and_condition_narrows_both_operands.ts"),
    );
}

#[test]
fn and_condition_false_branch_keeps_both_possibilities() {
    assert_clean(
        "and_condition_false_branch_keeps_both_possibilities.ts",
        fixture!("and_condition_false_branch_keeps_both_possibilities.ts"),
    );
}

#[test]
fn or_guard_clause_narrows_after_return() {
    assert_clean(
        "or_guard_clause_narrows_after_return.ts",
        fixture!("or_guard_clause_narrows_after_return.ts"),
    );
}

#[test]
fn or_condition_true_branch_joins_both_paths() {
    assert_clean(
        "or_condition_true_branch_joins_both_paths.ts",
        fixture!("or_condition_true_branch_joins_both_paths.ts"),
    );
}

#[test]
fn while_condition_narrows_the_loop_body() {
    assert_clean(
        "while_condition_narrows_the_loop_body.ts",
        fixture!("while_condition_narrows_the_loop_body.ts"),
    );
}

#[test]
fn for_condition_narrows_the_loop_body() {
    assert_clean(
        "for_condition_narrows_the_loop_body.ts",
        fixture!("for_condition_narrows_the_loop_body.ts"),
    );
}

#[test]
fn else_branch_that_exits_carries_the_true_narrowing() {
    assert_clean(
        "else_branch_that_exits_carries_the_true_narrowing.ts",
        fixture!("else_branch_that_exits_carries_the_true_narrowing.ts"),
    );
}

#[test]
fn assignment_in_branch_joins_back_to_a_narrow_type() {
    assert_clean(
        "assignment_in_branch_joins_back_to_a_narrow_type.ts",
        fixture!("assignment_in_branch_joins_back_to_a_narrow_type.ts"),
    );
}

#[test]
fn boolean_equality_narrows_both_branches() {
    assert_clean(
        "boolean_equality_narrows_both_branches.ts",
        fixture!("boolean_equality_narrows_both_branches.ts"),
    );
}

#[test]
fn unknown_equality_narrows_to_the_literal() {
    assert_clean(
        "unknown_equality_narrows_to_the_literal.ts",
        fixture!("unknown_equality_narrows_to_the_literal.ts"),
    );
}

#[test]
fn property_discriminant_wrong_member_is_an_error() {
    assert_one_error(
        "property_discriminant_wrong_member_is_an_error.ts",
        fixture!("property_discriminant_wrong_member_is_an_error.ts"),
        DiagnosticCode::PropertyDoesNotExist,
    );
}

#[test]
fn non_union_equality_narrows_to_the_literal_type() {
    assert_one_error(
        "non_union_equality_narrows_to_the_literal_type.ts",
        fixture!("non_union_equality_narrows_to_the_literal_type.ts"),
        DiagnosticCode::DeclaredTypeMismatch,
    );
}

#[test]
fn typeof_narrows_unknown_to_the_primitive() {
    assert_one_error(
        "typeof_narrows_unknown_to_the_primitive.ts",
        fixture!("typeof_narrows_unknown_to_the_primitive.ts"),
        DiagnosticCode::DeclaredTypeMismatch,
    );
}
