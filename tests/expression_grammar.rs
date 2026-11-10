use std::fs;
use std::path::PathBuf;

use tsox::{Diagnostic, DiagnosticCode, Severity, TypeChecker};

fn check_fixture(relative: &str) -> Vec<Diagnostic> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/expression-grammar")
        .join(relative);
    let source = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    TypeChecker::new()
        .check_source(&source, relative)
        .unwrap_or_else(|error| panic!("{relative} should parse: {error:?}"))
        .diagnostics
}

// Every diagnostic these fixtures produce must carry the tsc code the check was
// ported from and tsc's exact wording, because scripts/ts-diag-tool reads both back
// when it compares against the compiler itself.
fn assert_single_error(diagnostics: &[Diagnostic], code: DiagnosticCode, text: &str) {
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert_eq!(diagnostics[0].code, code, "got: {diagnostics:?}");
    assert_eq!(diagnostics[0].severity, Severity::Error);
    assert_eq!(diagnostics[0].message, text, "got: {diagnostics:?}");
}

fn assert_clean(diagnostics: &[Diagnostic]) {
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

// TS2790: deleting a required property is the one thing delete checks about the
// property itself; an optional property, an any object and a missing property (which
// reports TS2339 from the member lookup instead) all stay silent, as in tsc.
#[test]
fn deleting_a_required_property_is_reported() {
    let diagnostics = check_fixture("delete_required_property_is_reported.ts");
    assert_single_error(
        &diagnostics,
        DiagnosticCode::DeleteOperandRequired,
        "The operand of a 'delete' operator must be optional.",
    );
}

#[test]
fn deleting_an_optional_property_is_quiet() {
    assert_clean(&check_fixture("delete_optional_property_is_quiet.ts"));
}

#[test]
fn deleting_through_a_literal_computed_key_checks_optionality() {
    let diagnostics = check_fixture("delete_computed_literal_key_checks_optionality.ts");
    assert_single_error(
        &diagnostics,
        DiagnosticCode::DeleteOperandRequired,
        "The operand of a 'delete' operator must be optional.",
    );
}

#[test]
fn deleting_a_parenthesized_member_still_checks_optionality() {
    let diagnostics = check_fixture("delete_parenthesized_member_still_checks.ts");
    assert_single_error(
        &diagnostics,
        DiagnosticCode::DeleteOperandRequired,
        "The operand of a 'delete' operator must be optional.",
    );
}

#[test]
fn deleting_a_plain_name_is_not_a_property_reference() {
    let diagnostics = check_fixture("delete_identifier_reports_property_reference.ts");
    assert_single_error(
        &diagnostics,
        DiagnosticCode::DeleteOperandNotProperty,
        "The operand of a 'delete' operator must be a property reference.",
    );
}

#[test]
fn deleting_a_missing_property_reports_the_member_lookup_only() {
    let diagnostics = check_fixture("delete_missing_property_reports_its_code.ts");
    assert_single_error(
        &diagnostics,
        DiagnosticCode::PropertyDoesNotExist,
        "Property 'missing' does not exist on type 'Options'.",
    );
}

#[test]
fn deleting_from_an_any_object_is_quiet() {
    assert_clean(&check_fixture("delete_any_object_is_quiet.ts"));
}

// TS2695: a discarded comma part without an effect of its own; a call as the left
// part keeps its effect and stays quiet, as in tsc.
#[test]
fn a_discarded_comma_part_without_side_effects_is_reported() {
    let diagnostics = check_fixture("comma_left_literal_is_reported.ts");
    assert_eq!(diagnostics.len(), 3, "got: {diagnostics:?}");
    for diagnostic in &diagnostics {
        assert_eq!(diagnostic.code, DiagnosticCode::CommaLeftNoSideEffect);
        assert_eq!(
            diagnostic.message,
            "Left side of comma operator is unused and has no side effects."
        );
    }
}

#[test]
fn a_discarded_comma_part_with_a_call_is_quiet() {
    assert_clean(&check_fixture("comma_left_call_is_quiet.ts"));
}

// TS1345: an expression of type void cannot be tested for truthiness, in an if, a
// while, a ternary test and the operand of `!` alike.
#[test]
fn void_expressions_cannot_be_tested_for_truthiness() {
    let diagnostics = check_fixture("void_condition_cannot_be_tested.ts");
    assert_eq!(diagnostics.len(), 4, "got: {diagnostics:?}");
    for diagnostic in &diagnostics {
        assert_eq!(diagnostic.code, DiagnosticCode::VoidTruthiness);
        assert_eq!(
            diagnostic.message,
            "An expression of type 'void' cannot be tested for truthiness."
        );
    }
}

// TS2872/TS2873: tsc's syntactic rule on the condition expression itself. Null, the
// empty string and every allocating expression are one or the other; identifiers,
// booleans and the numbers 0 and 1 stay quiet, as in tsc.
#[test]
fn literal_conditions_classify_like_tsc() {
    let diagnostics = check_fixture("always_falsy_and_truthy_conditions.ts");
    assert_eq!(diagnostics.len(), 4, "got: {diagnostics:?}");
    let expected = [
        (DiagnosticCode::AlwaysFalsyExpression, "This kind of expression is always falsy."),
        (DiagnosticCode::AlwaysFalsyExpression, "This kind of expression is always falsy."),
        (DiagnosticCode::AlwaysTruthyExpression, "This kind of expression is always truthy."),
        (DiagnosticCode::AlwaysTruthyExpression, "This kind of expression is always truthy."),
    ];
    for (diagnostic, (code, text)) in diagnostics.iter().zip(expected) {
        assert_eq!(diagnostic.code, code, "got: {diagnostics:?}");
        assert_eq!(diagnostic.message, text);
    }
}

#[test]
fn the_numbers_zero_and_one_and_booleans_stay_quiet() {
    assert_clean(&check_fixture("numeric_zero_and_one_are_quiet.ts"));
}

#[test]
fn an_undefined_typed_condition_is_not_a_void_truthiness() {
    assert_clean(&check_fixture("undefined_typed_condition_is_quiet.ts"));
}
