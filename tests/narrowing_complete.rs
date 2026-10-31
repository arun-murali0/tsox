use tsox::{DiagnosticCode, TypeChecker};

// `in`, `instanceof` and loop-exit narrowing. Each fixture is valid TypeScript unless
// its file notes one deliberate error, so any other diagnostic is a false positive.

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!("fixtures/narrowing-complete/", $name))
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
fn in_operator_narrows_both_branches() {
    assert_clean(
        "in_operator_narrows_both_branches.ts",
        fixture!("in_operator_narrows_both_branches.ts"),
    );
}

#[test]
fn in_operator_keeps_a_member_with_an_optional_property() {
    assert_clean(
        "in_operator_keeps_a_member_with_an_optional_property.ts",
        fixture!("in_operator_keeps_a_member_with_an_optional_property.ts"),
    );
}

#[test]
fn in_operator_inside_an_and_condition() {
    assert_clean(
        "in_operator_inside_an_and_condition.ts",
        fixture!("in_operator_inside_an_and_condition.ts"),
    );
}

#[test]
fn in_operator_on_a_lone_object_leaves_it_alone() {
    assert_clean(
        "in_operator_on_a_lone_object_leaves_it_alone.ts",
        fixture!("in_operator_on_a_lone_object_leaves_it_alone.ts"),
    );
}

#[test]
fn instanceof_narrows_a_class_union() {
    assert_clean(
        "instanceof_narrows_a_class_union.ts",
        fixture!("instanceof_narrows_a_class_union.ts"),
    );
}

#[test]
fn instanceof_narrows_a_parent_to_its_subclass() {
    assert_clean(
        "instanceof_narrows_a_parent_to_its_subclass.ts",
        fixture!("instanceof_narrows_a_parent_to_its_subclass.ts"),
    );
}

#[test]
fn while_loop_that_cannot_break_narrows_after_the_loop() {
    assert_clean(
        "while_loop_that_cannot_break_narrows_after_the_loop.ts",
        fixture!("while_loop_that_cannot_break_narrows_after_the_loop.ts"),
    );
}

#[test]
fn for_loop_that_cannot_break_narrows_after_the_loop() {
    assert_clean(
        "for_loop_that_cannot_break_narrows_after_the_loop.ts",
        fixture!("for_loop_that_cannot_break_narrows_after_the_loop.ts"),
    );
}

#[test]
fn loop_test_narrowing_does_not_leak_into_a_following_branch() {
    assert_clean(
        "loop_test_narrowing_does_not_leak_into_a_following_branch.ts",
        fixture!("loop_test_narrowing_does_not_leak_into_a_following_branch.ts"),
    );
}

#[test]
fn in_operator_wrong_member_is_an_error() {
    assert_one_error(
        "in_operator_wrong_member_is_an_error.ts",
        fixture!("in_operator_wrong_member_is_an_error.ts"),
        DiagnosticCode::PropertyDoesNotExist,
    );
}

#[test]
fn instanceof_false_branch_drops_the_class() {
    assert_one_error(
        "instanceof_false_branch_drops_the_class.ts",
        fixture!("instanceof_false_branch_drops_the_class.ts"),
        DiagnosticCode::PropertyDoesNotExist,
    );
}

#[test]
fn while_loop_with_a_break_does_not_narrow_after_the_loop() {
    assert_one_error(
        "while_loop_with_a_break_does_not_narrow_after_the_loop.ts",
        fixture!("while_loop_with_a_break_does_not_narrow_after_the_loop.ts"),
        DiagnosticCode::DeclaredTypeMismatch,
    );
}

// A standard library class the file does not declare. `Date` and `Error` have no type
// here, so `unknown` narrows to `any` and the members are not an error.
#[test]
fn instanceof_date_narrows_unknown() {
    assert_clean(
        "instanceof_date_narrows_unknown.ts",
        fixture!("instanceof_date_narrows_unknown.ts"),
    );
}

#[test]
fn instanceof_error_narrows_unknown() {
    assert_clean(
        "instanceof_error_narrows_unknown.ts",
        fixture!("instanceof_error_narrows_unknown.ts"),
    );
}

#[test]
fn instanceof_promise_leaves_the_function_member() {
    assert_clean(
        "instanceof_promise_leaves_the_function_member.ts",
        fixture!("instanceof_promise_leaves_the_function_member.ts"),
    );
}

#[test]
fn instanceof_array_separates_an_array_member() {
    assert_clean(
        "instanceof_array_separates_an_array_member.ts",
        fixture!("instanceof_array_separates_an_array_member.ts"),
    );
}
