use std::fs;
use std::path::{Path, PathBuf};

use tsox::{Diagnostic, DiagnosticCode, Severity, TypeChecker};

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn check_fixture(relative: &str) -> Vec<Diagnostic> {
    let path = manifest_dir().join("tests/fixtures").join(relative);
    let source = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    let name = relative.rsplit('/').next().unwrap_or(relative);
    TypeChecker::new()
        .check_source(&source, name)
        .unwrap_or_else(|error| panic!("{relative} should parse: {error:?}"))
        .diagnostics
}

fn collect_ts_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_ts_files(&path, out);
        } else if path.extension().is_some_and(|extension| extension == "ts") {
            out.push(path);
        }
    }
}

fn assert_reports(relative: &str, code: DiagnosticCode, severity: Severity) {
    let diagnostics = check_fixture(relative);
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == code && d.severity == severity),
        "{relative}: expected {code} ({severity:?}), got: {diagnostics:?}"
    );
}

// Guards the convention documented in diagnostic_codes.rs: 1000s codes are real
// errors, 9000s codes are "not yet implemented" warnings. A new diagnostic that
// is emitted at the wrong severity for its code band, or with a malformed code,
// fails here for every fixture in the repo, not only in a test written for it.
#[test]
fn every_emitted_code_is_well_formed_and_matches_its_severity() {
    let mut files = Vec::new();
    collect_ts_files(&manifest_dir().join("tests/fixtures"), &mut files);
    assert!(!files.is_empty(), "no fixtures found");

    let mut seen = 0usize;
    for file in files {
        let Ok(source) = fs::read_to_string(&file) else {
            continue;
        };
        let name = file
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("fixture.ts");
        let Ok(result) = TypeChecker::new().check_source(&source, name) else {
            continue;
        };

        for diagnostic in result.diagnostics {
            seen += 1;
            let code = diagnostic.code.as_str();

            let well_formed = code.strip_prefix("TSR").is_some_and(|number| {
                number.len() == 4 && number.bytes().all(|b| b.is_ascii_digit())
            });
            assert!(well_formed, "{name}: malformed code {code:?}");

            let expected_prefix = match diagnostic.severity {
                Severity::Error => "TSR1",
                Severity::Warning => "TSR9",
            };
            assert!(
                code.starts_with(expected_prefix),
                "{name}: {code} was emitted as {:?}, expected a {expected_prefix}xxx code",
                diagnostic.severity
            );
        }
    }
    assert!(seen > 0, "no diagnostics were produced by any fixture");
}

// Codes are documented as stable identifiers, so the ones the generics work
// introduced are pinned here. Renumbering one has to be a deliberate edit.
#[test]
fn generics_diagnostic_codes_are_stable() {
    assert_eq!(
        DiagnosticCode::TypeArgumentConstraintViolation.as_str(),
        "TSR1007"
    );
    assert_eq!(
        DiagnosticCode::UnresolvableTypeParameterConstraint.as_str(),
        "TSR9014"
    );
    assert_eq!(
        DiagnosticCode::TypeArgumentCountMismatch.as_str(),
        "TSR1501"
    );
    assert_eq!(DiagnosticCode::TypeIsNotGeneric.as_str(), "TSR1502");
}

#[test]
fn constraint_violation_reports_its_code() {
    assert_reports(
        "generics-tier1/constraint_violation_is_caught.ts",
        DiagnosticCode::ArgumentNotAssignable,
        Severity::Error,
    );
    assert_reports(
        "generics-tier1/explicit_type_argument_violates_constraint.ts",
        DiagnosticCode::TypeArgumentConstraintViolation,
        Severity::Error,
    );
}

#[test]
fn unresolvable_constraint_reports_its_code_as_a_warning() {
    assert_reports(
        "generics-tier1/generic_unresolvable_constraint_is_reported.ts",
        DiagnosticCode::UnresolvableTypeParameterConstraint,
        Severity::Warning,
    );
}

#[test]
fn generic_call_mismatches_report_their_codes() {
    assert_reports(
        "generics-tier1/generic_return_type_mismatch_is_caught.ts",
        DiagnosticCode::DeclaredTypeMismatch,
        Severity::Error,
    );
    // `wrap<string>(5)` is a bad *argument*, so this is TSR1002 (tsc's TS2345),
    // not a declared-type mismatch.
    assert_reports(
        "generics-tier1/explicit_type_argument_overrides_inference.ts",
        DiagnosticCode::ArgumentNotAssignable,
        Severity::Error,
    );
    assert_reports(
        "generics-tier1/multi_candidate_with_no_common_type_is_still_caught.ts",
        DiagnosticCode::ArgumentNotAssignable,
        Severity::Error,
    );
}

#[test]
fn type_parameter_outside_its_declaration_is_an_unresolved_identifier() {
    assert_reports(
        "generics-tier1/type_param_does_not_leak_past_its_declaration.ts",
        DiagnosticCode::UnresolvedIdentifier,
        Severity::Error,
    );
}

#[test]
fn generic_interface_and_alias_mismatches_report_their_codes() {
    for fixture in [
        "generics-tier2/generic_interface_substitution_mismatch_is_caught.ts",
        "generics-tier2/generic_type_alias_substitution_mismatch_is_caught.ts",
        "generics-tier2/two_instantiations_of_the_same_interface_stay_distinct.ts",
    ] {
        assert_reports(
            fixture,
            DiagnosticCode::DeclaredTypeMismatch,
            Severity::Error,
        );
    }
}

#[test]
fn type_argument_arity_problems_report_their_codes() {
    assert_reports(
        "generics-tier2/type_argument_count_mismatch_is_reported.ts",
        DiagnosticCode::TypeArgumentCountMismatch,
        Severity::Error,
    );
    assert_reports(
        "generics-tier2/too_many_type_arguments_is_reported.ts",
        DiagnosticCode::TypeArgumentCountMismatch,
        Severity::Error,
    );
    assert_reports(
        "generics-tier2/type_arguments_on_a_non_generic_type_are_reported.ts",
        DiagnosticCode::TypeIsNotGeneric,
        Severity::Error,
    );
    // A bare `Box` for `Box<T>` is a missing type argument, an error in tsc too.
    assert_reports(
        "generics-tier2/bare_generic_reference_without_type_arguments_still_resolves.ts",
        DiagnosticCode::TypeArgumentCountMismatch,
        Severity::Error,
    );
    assert_reports(
        "generics-tier2/type_argument_violates_constraint.ts",
        DiagnosticCode::TypeArgumentConstraintViolation,
        Severity::Error,
    );
    assert_reports(
        "generics-tier2/type_argument_violates_alias_constraint.ts",
        DiagnosticCode::TypeArgumentConstraintViolation,
        Severity::Error,
    );
}
