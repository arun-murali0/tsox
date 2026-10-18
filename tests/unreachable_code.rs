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

// Filters to UnreachableCode specifically, rather than asserting the fixture's
// total diagnostic count: a `throw` fixture also hits this checker's separate,
// pre-existing gap around throw statements (see statements/mod.rs), and that
// warning is not this test's concern. What every fixture in the "flagged" group
// must produce is exactly one UnreachableCode (one, not one per statement in
// the dead region), regardless of what else the fixture happens to trip.
fn assert_reported_once(diagnostics: &[Diagnostic]) {
    let unreachable: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.code == DiagnosticCode::UnreachableCode)
        .collect();
    assert_eq!(
        unreachable.len(),
        1,
        "expected exactly one UnreachableCode diagnostic, got: {diagnostics:?}"
    );
    assert_eq!(unreachable[0].severity, Severity::Error);
}

#[test]
fn code_after_return_is_unreachable() {
    let source = include_str!("fixtures/unreachable-code/code_after_return_is_unreachable.ts");
    assert_reported_once(&check(source, "code_after_return_is_unreachable.ts"));
}

#[test]
fn code_after_throw_is_unreachable() {
    let source = include_str!("fixtures/unreachable-code/code_after_throw_is_unreachable.ts");
    assert_reported_once(&check(source, "code_after_throw_is_unreachable.ts"));
}

#[test]
fn code_after_return_inside_a_nested_block_is_unreachable() {
    let source = include_str!(
        "fixtures/unreachable-code/code_after_return_inside_nested_block_is_unreachable.ts"
    );
    assert_reported_once(&check(
        source,
        "code_after_return_inside_nested_block_is_unreachable.ts",
    ));
}

#[test]
fn trailing_code_is_unreachable_only_once_both_if_branches_exit() {
    let source = include_str!(
        "fixtures/unreachable-code/both_branches_exit_then_trailing_code_is_unreachable.ts"
    );
    assert_reported_once(&check(
        source,
        "both_branches_exit_then_trailing_code_is_unreachable.ts",
    ));
}

// The core reason this pass asks the graph rather than the AST: three dead
// statements in a row still produce one diagnostic, at the first of them, not
// three; the other two are never even visited, once the first one stops the
// walk (see visit_statements in src/bridge/unreachable_code.rs).
#[test]
fn a_dead_region_is_reported_once_not_per_statement() {
    let source = include_str!(
        "fixtures/unreachable-code/unreachable_region_is_reported_once_not_per_statement.ts"
    );
    assert_reported_once(&check(
        source,
        "unreachable_region_is_reported_once_not_per_statement.ts",
    ));
}

// The case this pass must not get wrong: a guard clause's own branch exits, but
// the code after the `if` is reached through the *other* branch, so it is live
// and must not be flagged. Asserts the fixture produces no diagnostics at all,
// not just no UnreachableCode, since this fixture has nothing else in it that
// should trip any other gap either.
#[test]
fn code_reachable_through_the_other_branch_is_not_flagged() {
    let source =
        include_str!("fixtures/unreachable-code/guard_clause_fallthrough_is_not_flagged.ts");
    let diagnostics = check(source, "guard_clause_fallthrough_is_not_flagged.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

// Verified against the real graph, not assumed (see the doc comment on
// check_unreachable_code in src/bridge/unreachable_code.rs): a function
// declaration placed after an unconditional return is not flagged. oxc's
// builder places a hoisted declaration's node in the function's entry block
// rather than at its textual position, so it never lands in a dead block to
// begin with. This matches tsc's own TS7027, which exempts a function
// declaration for the same underlying reason (hoisting), even though the
// mechanism here is different: nothing in this file special-cases it.
#[test]
fn an_unreachable_function_declaration_is_not_flagged() {
    let source = include_str!(
        "fixtures/unreachable-code/function_declaration_after_return_is_not_flagged.ts"
    );
    let diagnostics = check(
        source,
        "function_declaration_after_return_is_not_flagged.ts",
    );
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}
