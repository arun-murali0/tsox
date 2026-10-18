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
fn generic_identity_infers_return_type_per_call() {
    let source = include_str!("fixtures/generics-tier1/generic_identity_infers_return_type.ts");
    let diagnostics = check(source, "generic_identity_infers_return_type.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn generic_return_type_mismatch_is_caught() {
    let source = include_str!("fixtures/generics-tier1/generic_return_type_mismatch_is_caught.ts");
    let diagnostics = check(source, "generic_return_type_mismatch_is_caught.ts");
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
fn generic_multiple_type_params_infer_independently() {
    let source =
        include_str!("fixtures/generics-tier1/generic_multiple_type_params_infer_independently.ts");
    let diagnostics = check(
        source,
        "generic_multiple_type_params_infer_independently.ts",
    );
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn generic_array_element_type_is_inferred() {
    let source = include_str!("fixtures/generics-tier1/generic_array_element_type_is_inferred.ts");
    let diagnostics = check(source, "generic_array_element_type_is_inferred.ts");
    // This used to assert no diagnostics. d91f583 ("Include undefined on array
    // element access consistently") made `items[0]` include `| undefined` for
    // *every* array, including a generic `T[]`, so `firstElement`'s body now
    // honestly returns `T | undefined` against a declared `T`. That's the one
    // expected diagnostic here; it does not affect what this test actually
    // checks, which is that `T` is still inferred correctly at the call site
    // (`const n: number = firstElement([1, 2, 3])` still needs no diagnostic
    // of its own).
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic (the honest element | undefined vs T mismatch), got: {diagnostics:?}"
    );
    assert!(
        diagnostics[0].message.contains("is not assignable to type"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn generic_type_param_usable_inside_body() {
    let source = include_str!("fixtures/generics-tier1/generic_type_param_usable_inside_body.ts");
    let diagnostics = check(source, "generic_type_param_usable_inside_body.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn two_generic_functions_share_type_param_name_without_cross_contamination() {
    let source =
        include_str!("fixtures/generics-tier1/two_generic_functions_share_type_param_name.ts");
    let diagnostics = check(source, "two_generic_functions_share_type_param_name.ts");
    // This used to assert no diagnostics. Same cause as
    // generic_array_element_type_is_inferred: d91f583 made `firstOf`'s
    // `items[0]` include `| undefined`, so its body now mismatches its
    // declared `T`. That one diagnostic is expected and orthogonal to what
    // this test actually verifies: that resolving `firstOf`'s `T` and
    // `identity`'s `T` stay independent. Cross-contamination would show up
    // as a second, unexpected diagnostic on `const b: string =
    // identity("hello")` (e.g. `T` resolved to `number` there instead of
    // `string`), so asserting exactly one diagnostic, and that it's the
    // known `firstOf` mismatch, still proves no leak occurred.
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic (the honest element | undefined vs T mismatch in firstOf), got: {diagnostics:?}"
    );
    assert!(
        diagnostics[0].message.contains("is not assignable to type"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn type_param_does_not_leak_past_its_declaration() {
    let source =
        include_str!("fixtures/generics-tier1/type_param_does_not_leak_past_its_declaration.ts");
    let diagnostics = check(source, "type_param_does_not_leak_past_its_declaration.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic either way, got: {diagnostics:?}"
    );
    assert_eq!(diagnostics[0].severity, Severity::Error);
    assert!(
        diagnostics[0].message.contains("Cannot find name 'T'"),
        "expected a 'Cannot find name' error proving T did not leak, got: {diagnostics:?}"
    );
}

#[test]
fn multi_candidate_widens_to_common_supertype() {
    let source =
        include_str!("fixtures/generics-tier1/multi_candidate_widens_to_common_supertype.ts");
    let diagnostics = check(source, "multi_candidate_widens_to_common_supertype.ts");
    assert!(
        diagnostics.is_empty(),
        "expected T to widen to Base (Extended's supertype), got: {diagnostics:?}"
    );
}

#[test]
fn multi_candidate_with_no_common_type_is_still_caught() {
    let source = include_str!(
        "fixtures/generics-tier1/multi_candidate_with_no_common_type_is_still_caught.ts"
    );
    let diagnostics = check(
        source,
        "multi_candidate_with_no_common_type_is_still_caught.ts",
    );
    assert_eq!(
        diagnostics.len(),
        1,
        "number and string share no common type; expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert!(
        diagnostics[0].message.contains("not assignable"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn constraint_is_satisfied_and_usable_in_body() {
    let source =
        include_str!("fixtures/generics-tier1/constraint_is_satisfied_and_usable_in_body.ts");
    let diagnostics = check(source, "constraint_is_satisfied_and_usable_in_body.ts");
    assert!(
        diagnostics.is_empty(),
        "Box satisfies HasLength (extra fields are fine) and value.length should be \
         usable inside logLength's own body via T's constraint, got: {diagnostics:?}"
    );
}

#[test]
fn constraint_violation_is_caught() {
    let source = include_str!("fixtures/generics-tier1/constraint_violation_is_caught.ts");
    let diagnostics = check(source, "constraint_violation_is_caught.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "NoLength is missing 'length' and does not satisfy HasLength; expected exactly \
         one diagnostic, got: {diagnostics:?}"
    );
    assert!(
        diagnostics[0].message.contains(
            "Argument of type 'NoLength' is not assignable to parameter of type 'HasLength'"
        ),
        "got: {diagnostics:?}"
    );
    assert!(
        diagnostics[0]
            .message
            .contains("Property 'length' is missing in type 'NoLength'"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn union_parameter_infers_the_type_parameter_from_the_argument() {
    let source = include_str!("fixtures/generics-tier1/generic_union_parameter_inference.ts");
    let diagnostics = check(source, "generic_union_parameter_inference.ts");
    // `unwrap(5)` infers T = number from `T | undefined`, and `unwrap(mixed)`
    // infers T = number | string from a union argument in one step. Only the last
    // line, assigning `number | undefined` to a string, is a real error. Without
    // the union rule T stays unbound (unknown) and the first two lines become
    // false positives.
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly the deliberate mismatch, got: {diagnostics:?}"
    );
    assert_eq!(diagnostics[0].severity, Severity::Error);
    assert!(
        diagnostics[0].message.contains("not assignable"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn unresolvable_constraint_is_reported_instead_of_silently_dropped() {
    let source =
        include_str!("fixtures/generics-tier1/generic_unresolvable_constraint_is_reported.ts");
    let diagnostics = check(source, "generic_unresolvable_constraint_is_reported.ts");
    // The tuple bound `[number, string]` is not a type the resolver supports yet, so
    // the bound cannot be resolved and `T` is left unconstrained. That must be said
    // out loud, as a warning, not swallowed. It is not an error: the code is valid.
    // (This used to use a generic interface bound, `Wrapper<number>`, but generic
    // interfaces are now supported and resolve fine.)
    let constraint_warnings: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.message.contains("constraint of type parameter 'T'"))
        .collect();
    assert_eq!(
        constraint_warnings.len(),
        1,
        "expected one unresolvable-constraint warning, got: {diagnostics:?}"
    );
    assert_eq!(constraint_warnings[0].severity, Severity::Warning);
    assert!(
        diagnostics.iter().all(|d| d.severity != Severity::Error),
        "valid code must not gain errors, got: {diagnostics:?}"
    );
}

#[test]
fn a_resolvable_constraint_produces_no_warning() {
    let source =
        include_str!("fixtures/generics-tier1/constraint_is_satisfied_and_usable_in_body.ts");
    let diagnostics = check(source, "constraint_is_satisfied_and_usable_in_body.ts");
    assert!(
        diagnostics
            .iter()
            .all(|d| !d.message.contains("constraint of type parameter")),
        "got: {diagnostics:?}"
    );
}

#[test]
fn substituting_into_an_object_return_type_keeps_it_comparable() {
    let source =
        include_str!("fixtures/generics-tier1/generic_return_object_substitution_stays_sorted.ts");
    let diagnostics = check(source, "generic_return_object_substitution_stays_sorted.ts");
    // `{ second: T; first: string }` is rebuilt with T replaced by number, and the
    // rebuilt object must still line up property for property with a target that
    // lists them in the other order.
    assert!(
        diagnostics.is_empty(),
        "expected no diagnostics, got: {diagnostics:?}"
    );
}

#[test]
fn explicit_type_argument_is_used_when_given() {
    let source = include_str!("fixtures/generics-tier1/explicit_type_argument_is_used.ts");
    let diagnostics = check(source, "explicit_type_argument_is_used.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn explicit_type_argument_overrides_argument_driven_inference() {
    let source =
        include_str!("fixtures/generics-tier1/explicit_type_argument_overrides_inference.ts");
    let diagnostics = check(source, "explicit_type_argument_overrides_inference.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic (the explicit <string> should have \
         locked T, so the number argument no longer matches), got: {diagnostics:?}"
    );
    assert!(
        diagnostics[0].message.contains("not assignable"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn explicit_type_argument_is_checked_against_its_constraint() {
    let source =
        include_str!("fixtures/generics-tier1/explicit_type_argument_violates_constraint.ts");
    let diagnostics = check(source, "explicit_type_argument_violates_constraint.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert!(
        diagnostics[0]
            .message
            .contains("does not satisfy the constraint"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn generic_function_default_is_used_when_argument_inference_has_no_candidate() {
    // The generic parameter occurs in the return type, but no argument can infer it.
    // The declared default must survive signature construction and be applied at the call.
    let diagnostics = check(
        "declare function make<T = string>(): T; const text: string = make();",
        "generic_function_default_is_used_when_argument_inference_has_no_candidate.ts",
    );
    assert!(
        diagnostics.is_empty(),
        "expected T's string default to be used, got: {diagnostics:?}"
    );
}

#[test]
fn generic_function_type_default_is_used_for_a_nested_signature() {
    let diagnostics = check(
        "type Factory = <T = string>() => T; declare const make: Factory; const text: string = make();",
        "generic_function_type_default_is_used_for_a_nested_signature.ts",
    );
    assert!(
        diagnostics.is_empty(),
        "expected nested generic signature metadata to preserve its default, got: {diagnostics:?}"
    );
}
