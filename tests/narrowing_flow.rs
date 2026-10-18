use tsox::{Diagnostic, Severity, TypeChecker};

// Problem: narrowing was an AST walk that handled if/while/for/switch only. do-while,
// for-of, for-in, try, throw and labeled statements were reported as not yet checked and
// their bodies never walked; `break` and `continue` carried nothing out of a loop; a
// loop body was walked once, so an assignment at its bottom never reached its top; a
// closure saw every narrowing from where it was written; and a property path such as
// `user.address` could not be narrowed at all.
// Now: each fixture below was run through real tsc, and a test expects the same number of
// errors tsc gave. The statement fixtures must also produce no warning, so a statement
// that fell back to "not yet checked" fails them even when it happens to add no error.
// switch_fallthrough_merges_both_cases.ts is not tested: tsc's TS7029 (fallthrough case
// lint) is a separate check this checker does not have.

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

fn errors(diagnostics: &[Diagnostic]) -> Vec<&Diagnostic> {
    diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect()
}

fn warnings(diagnostics: &[Diagnostic]) -> Vec<&Diagnostic> {
    diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Warning)
        .collect()
}

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!("fixtures/narrowing-flow/", $name, ".ts"))
    };
}

// an assignment inside a condition narrows the assigned variable
#[test]
fn assignment_inside_a_condition() {
    let diagnostics = check(
        fixture!("assignment_inside_a_condition"),
        "assignment_inside_a_condition.ts",
    );
    assert!(
        errors(&diagnostics).is_empty(),
        "expected no errors, got: {diagnostics:?}"
    );
}

// a break carries its own state out of the loop
#[test]
fn break_carries_its_state_out_of_the_loop() {
    let diagnostics = check(
        fixture!("break_carries_its_state_out_of_the_loop"),
        "break_carries_its_state_out_of_the_loop.ts",
    );
    assert!(
        errors(&diagnostics).is_empty(),
        "expected no errors, got: {diagnostics:?}"
    );
}

// a closure keeps the narrowing of a const
#[test]
fn closure_keeps_narrowing_of_a_const() {
    let diagnostics = check(
        fixture!("closure_keeps_narrowing_of_a_const"),
        "closure_keeps_narrowing_of_a_const.ts",
    );
    assert!(
        errors(&diagnostics).is_empty(),
        "expected no errors, got: {diagnostics:?}"
    );
}

// a continue hands its state to the next pass, not to the code after the loop
#[test]
fn continue_skips_the_rest_of_the_body() {
    let diagnostics = check(
        fixture!("continue_skips_the_rest_of_the_body"),
        "continue_skips_the_rest_of_the_body.ts",
    );
    assert!(
        errors(&diagnostics).is_empty(),
        "expected no errors, got: {diagnostics:?}"
    );
}

// a finally block does not change what the try block saw
#[test]
fn finally_does_not_leak_into_the_try() {
    let diagnostics = check(
        fixture!("finally_does_not_leak_into_the_try"),
        "finally_does_not_leak_into_the_try.ts",
    );
    assert!(
        errors(&diagnostics).is_empty(),
        "expected no errors, got: {diagnostics:?}"
    );
}

// a guard narrows the element of a for-of
#[test]
fn for_of_guard_narrows_the_element() {
    let diagnostics = check(
        fixture!("for_of_guard_narrows_the_element"),
        "for_of_guard_narrows_the_element.ts",
    );
    assert!(
        errors(&diagnostics).is_empty(),
        "expected no errors, got: {diagnostics:?}"
    );
}

// a loop that assigns nothing narrowed keeps the narrowing from before it
#[test]
fn loop_without_assignment_keeps_narrowing() {
    let diagnostics = check(
        fixture!("loop_without_assignment_keeps_narrowing"),
        "loop_without_assignment_keeps_narrowing.ts",
    );
    assert!(
        errors(&diagnostics).is_empty(),
        "expected no errors, got: {diagnostics:?}"
    );
}

// a guard before a loop still holds inside it and after it
#[test]
fn narrowing_after_a_loop_with_a_guard() {
    let diagnostics = check(
        fixture!("narrowing_after_a_loop_with_a_guard"),
        "narrowing_after_a_loop_with_a_guard.ts",
    );
    assert!(
        errors(&diagnostics).is_empty(),
        "expected no errors, got: {diagnostics:?}"
    );
}

// an or-guard that returns narrows both operands afterwards
#[test]
fn or_guard_narrows_both_operands() {
    let diagnostics = check(
        fixture!("or_guard_narrows_both_operands"),
        "or_guard_narrows_both_operands.ts",
    );
    assert!(
        errors(&diagnostics).is_empty(),
        "expected no errors, got: {diagnostics:?}"
    );
}

// a property path is narrowed by a null check
#[test]
fn property_path_is_narrowed() {
    let diagnostics = check(
        fixture!("property_path_is_narrowed"),
        "property_path_is_narrowed.ts",
    );
    assert!(
        errors(&diagnostics).is_empty(),
        "expected no errors, got: {diagnostics:?}"
    );
}

// a guard that throws narrows what follows
#[test]
fn throw_guard_narrows_what_follows() {
    let diagnostics = check(
        fixture!("throw_guard_narrows_what_follows"),
        "throw_guard_narrows_what_follows.ts",
    );
    assert!(
        errors(&diagnostics).is_empty(),
        "expected no errors, got: {diagnostics:?}"
    );
}

// a closure loses the narrowing of a variable assigned after it is made
#[test]
fn closure_loses_narrowing_when_reassigned_after() {
    let diagnostics = check(
        fixture!("closure_loses_narrowing_when_reassigned_after"),
        "closure_loses_narrowing_when_reassigned_after.ts",
    );
    assert_eq!(
        errors(&diagnostics).len(),
        1,
        "expected the one error tsc reports, got: {diagnostics:?}"
    );
}

// a do-while body is checked, and its test cannot narrow it
#[test]
fn do_while_body_is_checked() {
    let diagnostics = check(
        fixture!("do_while_body_is_checked"),
        "do_while_body_is_checked.ts",
    );
    assert_eq!(
        errors(&diagnostics).len(),
        1,
        "expected the one error tsc reports, got: {diagnostics:?}"
    );
}

// a for-of body is checked
#[test]
fn for_of_body_is_checked() {
    let diagnostics = check(
        fixture!("for_of_body_is_checked"),
        "for_of_body_is_checked.ts",
    );
    assert_eq!(
        errors(&diagnostics).len(),
        1,
        "expected the one error tsc reports, got: {diagnostics:?}"
    );
}

// a labeled break joins the outer loop's exit with the loop's own
#[test]
fn labeled_break_leaves_the_outer_loop() {
    let diagnostics = check(
        fixture!("labeled_break_leaves_the_outer_loop"),
        "labeled_break_leaves_the_outer_loop.ts",
    );
    assert_eq!(
        errors(&diagnostics).len(),
        1,
        "expected the one error tsc reports, got: {diagnostics:?}"
    );
}

// an assignment at the bottom of a loop reaches its head
#[test]
fn loop_assignment_reaches_the_loop_head() {
    let diagnostics = check(
        fixture!("loop_assignment_reaches_the_loop_head"),
        "loop_assignment_reaches_the_loop_head.ts",
    );
    assert_eq!(
        errors(&diagnostics).len(),
        1,
        "expected the one error tsc reports, got: {diagnostics:?}"
    );
}

// assigning a property ends its narrowing
#[test]
fn property_path_narrowing_is_lost_on_assignment() {
    let diagnostics = check(
        fixture!("property_path_narrowing_is_lost_on_assignment"),
        "property_path_narrowing_is_lost_on_assignment.ts",
    );
    assert_eq!(
        errors(&diagnostics).len(),
        1,
        "expected the one error tsc reports, got: {diagnostics:?}"
    );
}

// a try body is checked
#[test]
fn try_body_is_checked() {
    let diagnostics = check(fixture!("try_body_is_checked"), "try_body_is_checked.ts");
    assert_eq!(
        errors(&diagnostics).len(),
        1,
        "expected the one error tsc reports, got: {diagnostics:?}"
    );
}

// None of the statement kinds above may fall back to the "not yet checked" warning.
#[test]
fn the_statement_fixtures_raise_no_warnings() {
    let diagnostics = check(
        fixture!("do_while_body_is_checked"),
        "do_while_body_is_checked.ts",
    );
    assert!(
        warnings(&diagnostics).is_empty(),
        "do_while_body_is_checked: unexpected warnings: {diagnostics:?}"
    );
    let diagnostics = check(
        fixture!("for_of_body_is_checked"),
        "for_of_body_is_checked.ts",
    );
    assert!(
        warnings(&diagnostics).is_empty(),
        "for_of_body_is_checked: unexpected warnings: {diagnostics:?}"
    );
    let diagnostics = check(
        fixture!("for_of_guard_narrows_the_element"),
        "for_of_guard_narrows_the_element.ts",
    );
    assert!(
        warnings(&diagnostics).is_empty(),
        "for_of_guard_narrows_the_element: unexpected warnings: {diagnostics:?}"
    );
    let diagnostics = check(fixture!("try_body_is_checked"), "try_body_is_checked.ts");
    assert!(
        warnings(&diagnostics).is_empty(),
        "try_body_is_checked: unexpected warnings: {diagnostics:?}"
    );
    let diagnostics = check(
        fixture!("throw_guard_narrows_what_follows"),
        "throw_guard_narrows_what_follows.ts",
    );
    assert!(
        warnings(&diagnostics).is_empty(),
        "throw_guard_narrows_what_follows: unexpected warnings: {diagnostics:?}"
    );
    let diagnostics = check(
        fixture!("labeled_break_leaves_the_outer_loop"),
        "labeled_break_leaves_the_outer_loop.ts",
    );
    assert!(
        warnings(&diagnostics).is_empty(),
        "labeled_break_leaves_the_outer_loop: unexpected warnings: {diagnostics:?}"
    );
    let diagnostics = check(
        fixture!("finally_does_not_leak_into_the_try"),
        "finally_does_not_leak_into_the_try.ts",
    );
    assert!(
        warnings(&diagnostics).is_empty(),
        "finally_does_not_leak_into_the_try: unexpected warnings: {diagnostics:?}"
    );
}
