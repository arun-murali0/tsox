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
fn generic_interface_substitution_checks_clean() {
    let source =
        include_str!("fixtures/generics-tier2/generic_interface_substitution_checks_clean.ts");
    let diagnostics = check(source, "generic_interface_substitution_checks_clean.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn generic_interface_substitution_mismatch_is_caught() {
    let source = include_str!(
        "fixtures/generics-tier2/generic_interface_substitution_mismatch_is_caught.ts"
    );
    let diagnostics = check(
        source,
        "generic_interface_substitution_mismatch_is_caught.ts",
    );
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert!(
        diagnostics[0].message.contains("not assignable"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn generic_type_alias_substitution_checks_clean() {
    let source =
        include_str!("fixtures/generics-tier2/generic_type_alias_substitution_checks_clean.ts");
    let diagnostics = check(source, "generic_type_alias_substitution_checks_clean.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn generic_type_alias_substitution_mismatch_is_caught() {
    let source = include_str!(
        "fixtures/generics-tier2/generic_type_alias_substitution_mismatch_is_caught.ts"
    );
    let diagnostics = check(
        source,
        "generic_type_alias_substitution_mismatch_is_caught.ts",
    );
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert!(
        diagnostics[0].message.contains("not assignable"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn two_instantiations_of_the_same_interface_stay_distinct() {
    let source = include_str!(
        "fixtures/generics-tier2/two_instantiations_of_the_same_interface_stay_distinct.ts"
    );
    let diagnostics = check(
        source,
        "two_instantiations_of_the_same_interface_stay_distinct.ts",
    );
    assert_eq!(
        diagnostics.len(),
        1,
        "Box<number> and Box<string> must not be interchangeable, got: {diagnostics:?}"
    );
    assert!(
        diagnostics[0].message.contains("not assignable"),
        "got: {diagnostics:?}"
    );
}

// tsc rejects a bare generic reference (TS2314), so tsox reports it as an error
// too. The reference is typed as the error type, so `box.value` in the body adds
// no second diagnostic.
#[test]
fn bare_generic_reference_without_type_arguments_is_reported_as_an_error() {
    let source = include_str!(
        "fixtures/generics-tier2/bare_generic_reference_without_type_arguments_still_resolves.ts"
    );
    let diagnostics = check(
        source,
        "bare_generic_reference_without_type_arguments_still_resolves.ts",
    );
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert_eq!(diagnostics[0].severity, Severity::Error);
    assert!(
        diagnostics[0]
            .message
            .contains("requires 1 type argument(s)"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn too_few_type_arguments_is_reported_as_an_error() {
    let source =
        include_str!("fixtures/generics-tier2/type_argument_count_mismatch_is_reported.ts");
    let diagnostics = check(source, "type_argument_count_mismatch_is_reported.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert_eq!(diagnostics[0].severity, Severity::Error);
    assert!(
        diagnostics[0]
            .message
            .contains("requires 2 type argument(s)"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn too_many_type_arguments_is_reported_as_an_error() {
    let source = include_str!("fixtures/generics-tier2/too_many_type_arguments_is_reported.ts");
    let diagnostics = check(source, "too_many_type_arguments_is_reported.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert_eq!(diagnostics[0].severity, Severity::Error);
    assert!(
        diagnostics[0]
            .message
            .contains("requires 1 type argument(s)"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn type_arguments_on_a_non_generic_type_are_reported_as_an_error() {
    let source = include_str!(
        "fixtures/generics-tier2/type_arguments_on_a_non_generic_type_are_reported.ts"
    );
    let diagnostics = check(
        source,
        "type_arguments_on_a_non_generic_type_are_reported.ts",
    );
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert_eq!(diagnostics[0].severity, Severity::Error);
    assert!(
        diagnostics[0].message.contains("is not generic"),
        "got: {diagnostics:?}"
    );
}

// `Box<number>` for `interface Box<T extends string>` (TS2344): the type argument
// does not satisfy the parameter's bound.
#[test]
fn type_argument_violating_an_interface_constraint_is_reported() {
    let source = include_str!("fixtures/generics-tier2/type_argument_violates_constraint.ts");
    let diagnostics = check(source, "type_argument_violates_constraint.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert_eq!(diagnostics[0].severity, Severity::Error);
    assert!(
        diagnostics[0]
            .message
            .contains("does not satisfy the constraint"),
        "got: {diagnostics:?}"
    );
}

// The same rule for a generic type alias, with an object-shaped bound.
#[test]
fn type_argument_violating_an_alias_constraint_is_reported() {
    let source = include_str!("fixtures/generics-tier2/type_argument_violates_alias_constraint.ts");
    let diagnostics = check(source, "type_argument_violates_alias_constraint.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert_eq!(diagnostics[0].severity, Severity::Error);
    assert!(
        diagnostics[0]
            .message
            .contains("does not satisfy the constraint"),
        "got: {diagnostics:?}"
    );
}

// Arguments that satisfy their bounds report nothing, including a type parameter
// passed on as an argument, which is left for the point of instantiation.
#[test]
fn type_arguments_that_satisfy_their_constraints_are_accepted() {
    let source = include_str!("fixtures/generics-tier2/type_argument_satisfies_constraint.ts");
    let diagnostics = check(source, "type_argument_satisfies_constraint.ts");
    assert!(
        diagnostics.iter().all(|d| d.severity != Severity::Error),
        "expected no errors, got: {diagnostics:?}"
    );
}

// `interface Second<A, B> { value: B }` never mentions A in its body, so the
// only way `Second<number, string>` binds B = string is by giving A its own
// slot. Only the `bad` line (value: 1) may error.
#[test]
fn unused_type_parameter_keeps_its_position() {
    let source =
        include_str!("fixtures/generics-tier2/unused_type_parameter_keeps_its_position.ts");
    let diagnostics = check(source, "unused_type_parameter_keeps_its_position.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "only the `bad` line should error, got: {diagnostics:?}"
    );
    assert!(
        diagnostics[0].message.contains("not assignable"),
        "got: {diagnostics:?}"
    );
}

// `Pair<number>` is valid for `interface Pair<A, B = string>` in tsc: no
// argument-count error, and B falls back to string so the `bad` line errors.
#[test]
fn omitted_type_argument_uses_its_default() {
    let source = include_str!("fixtures/generics-tier2/type_argument_default_is_applied.ts");
    let diagnostics = check(source, "type_argument_default_is_applied.ts");
    let errors: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert_eq!(errors.len(), 1, "got: {diagnostics:?}");
    assert!(
        errors[0].message.contains("not assignable"),
        "got: {diagnostics:?}"
    );
}

// A range (some trailing parameters have defaults) is reported as a range, the
// way tsc's TS2707 is, rather than as an exact count.
#[test]
fn count_range_message_names_the_range_when_defaults_exist() {
    let source = "interface Pair<A, B = string> { first: A; second: B; }\n\
                  const p: Pair<number, string, boolean> = { first: 1, second: \"x\" };\n";
    let diagnostics = check(source, "range.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert_eq!(diagnostics[0].severity, Severity::Error);
    assert!(
        diagnostics[0].message.contains("between 1 and 2"),
        "got: {diagnostics:?}"
    );
}

// `U extends T[]` is written in terms of another parameter, so it has to be
// compared after T is known. Compared raw, `number[]` was tested against the
// generic `T[]` and even the correct call was rejected. With T inferred as string
// from the second argument the bound becomes `string[]`, which `numbers` does not
// satisfy: one error, on the second call only.
#[test]
fn constraint_is_checked_with_the_other_parameters_substituted() {
    let source =
        include_str!("fixtures/generics-tier2/constraint_written_in_terms_of_another_parameter.ts");
    let diagnostics = check(
        source,
        "constraint_written_in_terms_of_another_parameter.ts",
    );
    let errors: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert_eq!(errors.len(), 1, "got: {diagnostics:?}");
    assert!(
        errors[0].message.contains(
            "Argument of type 'number[]' is not assignable to parameter of type 'string[]'"
        ),
        "got: {diagnostics:?}"
    );
}

// A type parameter used as a type argument is compared through its own bound: U
// extends string satisfies Box's `T extends string`, an unconstrained V does not.
// Previously both were skipped.
#[test]
fn type_parameter_argument_is_checked_through_its_bound() {
    let source = include_str!(
        "fixtures/generics-tier2/type_parameter_argument_is_checked_through_its_bound.ts"
    );
    let diagnostics = check(
        source,
        "type_parameter_argument_is_checked_through_its_bound.ts",
    );
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert_eq!(diagnostics[0].severity, Severity::Error);
    assert!(
        diagnostics[0].message.contains("Type 'V'")
            && diagnostics[0]
                .message
                .contains("does not satisfy the constraint"),
        "got: {diagnostics:?}"
    );
}
