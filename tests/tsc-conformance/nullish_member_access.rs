use tsox::{Diagnostic, DiagnosticCode, Severity, TypeChecker};

// Problem: reading a property of `string | null` was reported as "Property 'length' does
// not exist on type 'string | null'" (TS2339). tsc reports the cause, "'x' is possibly
// 'null'." (TS18047, or 18048 / 18049 for undefined / both), and names the object the way
// it is written; with no name to give (a call result) it says "Object is possibly 'null'."
// (TS2531).
// Now: each fixture is compared with the exact code and message tsc prints, and
// tests/fixtures/nullish-member-access is also run against real tsc by
// scripts/ts-diag-tool/compare.js.

fn check(source: &str, file_name: &str) -> Vec<(DiagnosticCode, String)> {
    let result = TypeChecker::new().check_source(source, file_name);
    assert!(result.is_ok(), "fixture should at least parse: {result:?}");
    let diagnostics: Vec<Diagnostic> = result
        .map(|checked| checked.diagnostics)
        .unwrap_or_default();
    for diagnostic in &diagnostics {
        assert_eq!(diagnostic.severity, Severity::Error, "{diagnostics:?}");
    }
    diagnostics
        .into_iter()
        .map(|diagnostic| (diagnostic.code, diagnostic.message))
        .collect()
}

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!("fixtures/nullish-member-access/", $name, ".ts"))
    };
}

fn assert_only(source: &str, file_name: &str, code: DiagnosticCode, message: &str) {
    assert_eq!(
        check(source, file_name),
        vec![(code, message.to_string())],
        "{file_name}"
    );
}

#[test]
fn a_member_of_a_possibly_null_value_is_possibly_null() {
    assert_only(
        fixture!("null_member_read_is_possibly_null"),
        "null_member_read_is_possibly_null.ts",
        DiagnosticCode::PossiblyNull,
        "'x' is possibly 'null'.",
    );
}

#[test]
fn a_member_of_a_possibly_undefined_value_is_possibly_undefined() {
    assert_only(
        fixture!("undefined_member_read_is_possibly_undefined"),
        "undefined_member_read_is_possibly_undefined.ts",
        DiagnosticCode::PossiblyUndefined,
        "'x' is possibly 'undefined'.",
    );
}

#[test]
fn a_value_that_may_be_either_names_both() {
    assert_only(
        fixture!("null_or_undefined_member_read_names_both"),
        "null_or_undefined_member_read_names_both.ts",
        DiagnosticCode::PossiblyNullOrUndefined,
        "'x' is possibly 'null' or 'undefined'.",
    );
}

#[test]
fn a_property_path_is_named_as_written() {
    assert_only(
        fixture!("property_path_is_named_as_written"),
        "property_path_is_named_as_written.ts",
        DiagnosticCode::PossiblyNull,
        "'user.address' is possibly 'null'.",
    );
}

#[test]
fn a_call_result_has_no_name_to_report() {
    assert_only(
        fixture!("call_result_has_no_name"),
        "call_result_has_no_name.ts",
        DiagnosticCode::ObjectPossiblyNull,
        "Object is possibly 'null'.",
    );
}

#[test]
fn a_method_call_on_a_nullable_value_is_reported() {
    assert_only(
        fixture!("method_call_on_nullable_is_reported"),
        "method_call_on_nullable_is_reported.ts",
        DiagnosticCode::PossiblyNull,
        "'service' is possibly 'null'.",
    );
}

#[test]
fn an_element_access_on_a_nullable_value_is_reported() {
    let found = check(
        fixture!("element_access_on_nullable_is_reported"),
        "element_access_on_nullable_is_reported.ts",
    );
    assert!(
        found.contains(&(
            DiagnosticCode::PossiblyNull,
            "'items' is possibly 'null'.".to_string()
        )),
        "got: {found:?}"
    );
}

#[test]
fn a_missing_property_is_still_reported_once_null_is_taken_away() {
    assert_eq!(
        check(
            fixture!("missing_property_is_still_reported_after_null"),
            "missing_property_is_still_reported_after_null.ts"
        ),
        vec![
            (
                DiagnosticCode::PossiblyNull,
                "'box' is possibly 'null'.".to_string()
            ),
            (
                DiagnosticCode::PropertyDoesNotExist,
                "Property 'b' does not exist on type 'Box'.".to_string()
            ),
        ]
    );
}

#[test]
fn an_optional_chain_and_a_guard_report_nothing() {
    assert_eq!(
        check(
            fixture!("optional_chain_is_not_reported"),
            "optional_chain_is_not_reported.ts"
        ),
        vec![]
    );
}
