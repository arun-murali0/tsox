use tsox::{DiagnosticCode, Severity, TypeChecker};

fn diagnostics(fixture: &str, file_name: &str) -> Vec<tsox::Diagnostic> {
    let result = TypeChecker::new().check_source(fixture, file_name);
    assert!(result.is_ok(), "fixture should at least parse: {result:?}");
    result.unwrap().diagnostics
}

fn assert_clean(name: &str) {
    let source = match name {
        "constraint_fallback" => {
            include_str!("fixtures/generic-overload-resolution/constraint_fallback.ts")
        }
        "independent_candidate_inference" => {
            include_str!("fixtures/generic-overload-resolution/independent_candidate_inference.ts")
        }
        "explicit_type_arguments" => {
            include_str!("fixtures/generic-overload-resolution/explicit_type_arguments.ts")
        }
        "multiple_argument_inference" => {
            include_str!("fixtures/generic-overload-resolution/multiple_argument_inference.ts")
        }
        "composite_parameter_inference" => {
            include_str!("fixtures/generic-overload-resolution/composite_parameter_inference.ts")
        }
        "contextual_callback_selection" => {
            include_str!("fixtures/generic-overload-resolution/contextual_callback_selection.ts")
        }
        "callback_return_drives_inference" => {
            include_str!("fixtures/generic-overload-resolution/callback_return_drives_inference.ts")
        }
        "first_applicable_overload_wins" => {
            include_str!("fixtures/generic-overload-resolution/first_applicable_overload_wins.ts")
        }
        "broad_before_narrow" => {
            include_str!("fixtures/generic-overload-resolution/broad_before_narrow.ts")
        }
        "arity_falls_through" => {
            include_str!("fixtures/generic-overload-resolution/arity_falls_through.ts")
        }
        "defaulted_generic_candidate" => {
            include_str!("fixtures/generic-overload-resolution/defaulted_generic_candidate.ts")
        }
        _ => unreachable!("unknown fixture"),
    };
    let found = diagnostics(source, &format!("{name}.ts"));
    assert!(
        found.is_empty(),
        "{name}: expected no diagnostics, got {found:?}"
    );
}

#[test]
fn a_constraint_failure_does_not_mask_a_later_non_generic_overload() {
    assert_clean("constraint_fallback");
}

#[test]
fn each_generic_candidate_gets_an_independent_inference_session() {
    assert_clean("independent_candidate_inference");
}

#[test]
fn explicit_type_arguments_participate_in_overload_applicability() {
    assert_clean("explicit_type_arguments");
}

#[test]
fn inference_combines_evidence_from_all_arguments_per_candidate() {
    assert_clean("multiple_argument_inference");
}

#[test]
fn generic_candidates_are_inferred_through_nested_object_properties() {
    assert_clean("composite_parameter_inference");
}

#[test]
fn callback_context_is_evaluated_per_overload_candidate() {
    assert_clean("contextual_callback_selection");
}

#[test]
fn callback_return_types_contribute_generic_candidates_per_overload() {
    assert_clean("callback_return_drives_inference");
}

#[test]
fn the_first_applicable_overload_keeps_source_order_precedence() {
    assert_clean("first_applicable_overload_wins");
}

#[test]
fn no_applicable_generic_overload_reports_ts2769() {
    let source = include_str!("fixtures/generic-overload-resolution/no_overload_matches.ts");
    let found = diagnostics(source, "no_overload_matches.ts");
    let errors: Vec<_> = found
        .iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .collect();
    assert_eq!(
        errors.len(),
        1,
        "expected one overload diagnostic, got {found:?}"
    );
    assert_eq!(errors[0].code, DiagnosticCode::NoOverloadMatches);
}

#[test]
fn source_order_is_preserved_even_when_a_later_overload_is_narrower() {
    assert_clean("broad_before_narrow");
}

#[test]
fn an_inapplicable_arity_candidate_does_not_mask_a_later_overload() {
    assert_clean("arity_falls_through");
}

#[test]
fn defaulted_generic_parameters_are_applied_during_candidate_probing() {
    assert_clean("defaulted_generic_candidate");
}
