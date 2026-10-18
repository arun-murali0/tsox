use tsox::{Diagnostic, DiagnosticCode, Severity, TypeChecker};

fn check(fixture_source: &str, file_name: &str) -> Vec<Diagnostic> {
    let checker = TypeChecker::new();
    let result = checker.check_source(fixture_source, file_name);
    assert!(result.is_ok(), "fixture should at least parse: {result:?}");
    result
        .map(|checked| checked.diagnostics)
        .unwrap_or_default()
}

// Filters to Severity::Error only, matching the pattern already used
// elsewhere in this test suite (e.g. structural_types.rs's `errors()`,
// recursive_types.rs). Warning-severity diagnostics are implementation-
// status markers ("this expression kind isn't checked yet"), not something
// a fixture's correctness should be judged on. Assignment expressions
// (`x = y`, `this.x = y`) in particular are not yet handled anywhere in
// infer_expression_type (see src/bridge/expressions/mod.rs's catch-all
// arm), a pre-existing, checker-wide gap unrelated to generics; it was
// simply invisible in these fixtures before generic class bodies were
// actually being checked at all (see check_class_declaration's now-fixed
// missing type-parameter scope push).
fn single_error(diagnostics: &[Diagnostic], code: DiagnosticCode) -> &Diagnostic {
    let errors: Vec<&Diagnostic> = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert_eq!(
        errors.len(),
        1,
        "expected exactly one error-severity diagnostic, got: {diagnostics:?}"
    );
    assert_eq!(errors[0].code, code, "got: {diagnostics:?}");
    diagnostics
        .iter()
        .find(|d| d.severity == Severity::Error)
        .unwrap()
}

// Before this, a class's own <T, ...> list was never shadowed the way an
// interface's or alias's is (namespace::resolve's type_params match returned
// None for every class), so `T` inside a class body just resolved as an
// unknown name, and Box<number> could never substitute anything.
#[test]
fn generic_class_field_is_substituted() {
    let source = include_str!("fixtures/generic-classes/generic_class_field_is_substituted.ts");
    let diagnostics = check(source, "generic_class_field_is_substituted.ts");
    single_error(&diagnostics, DiagnosticCode::ReturnTypeMismatch);
}

#[test]
fn generic_class_method_argument_is_checked() {
    let source =
        include_str!("fixtures/generic-classes/generic_class_method_argument_is_checked.ts");
    let diagnostics = check(source, "generic_class_method_argument_is_checked.ts");
    single_error(&diagnostics, DiagnosticCode::ArgumentNotAssignable);
}

// tsc: "Type 'Box<number>' is not assignable to type 'Box<string>'."
#[test]
fn two_instantiations_of_the_same_generic_class_stay_distinct() {
    let source = include_str!(
        "fixtures/generic-classes/two_instantiations_of_the_same_generic_class_stay_distinct.ts"
    );
    let diagnostics = check(
        source,
        "two_instantiations_of_the_same_generic_class_stay_distinct.ts",
    );
    let error = single_error(&diagnostics, DiagnosticCode::DeclaredTypeMismatch);
    assert!(error.message.contains("'Box<number>'"), "got: {error:?}");
    assert!(error.message.contains("'Box<string>'"), "got: {error:?}");
}

// The common recursive-generic-class shape: referring to itself with its own
// same type parameter. Same known limitation as the interface case (a
// self-reference with a *different*, fixed argument doesn't substitute
// correctly yet) applies here too and isn't exercised.
#[test]
fn generic_class_recursive_field_with_same_type_param_resolves() {
    let source = include_str!(
        "fixtures/generic-classes/generic_class_recursive_field_with_same_type_param_resolves.ts"
    );
    let diagnostics = check(
        source,
        "generic_class_recursive_field_with_same_type_param_resolves.ts",
    );
    single_error(&diagnostics, DiagnosticCode::ReturnTypeMismatch);
}

// tsc: "Generic type 'Box<T>' requires 1 type argument(s)." The existing
// count-check machinery in type_annotation.rs already worked off
// declared_type_param_arity/decl, which now cover classes too, so this needed
// no changes beyond the two namespace.rs edits.
#[test]
fn generic_class_type_argument_count_mismatch_is_reported() {
    let source = include_str!(
        "fixtures/generic-classes/generic_class_type_argument_count_mismatch_is_reported.ts"
    );
    let diagnostics = check(
        source,
        "generic_class_type_argument_count_mismatch_is_reported.ts",
    );
    single_error(&diagnostics, DiagnosticCode::TypeArgumentCountMismatch);
}

// check_class_declaration never pushed the class's own <T, ...> into scope
// before checking constructor/method bodies (only resolve_class did, at
// declare time, for field/param annotations), so a method other than the
// constructor that assigns `this.prop = someTParam` inside its own body hit
// an unresolved-name path instead of correctly treating T as the same
// GenericParameter node used everywhere else for this class.
#[test]
fn generic_class_method_assigns_this_property() {
    let source =
        include_str!("fixtures/generic-classes/generic_class_method_assigns_this_property.ts");
    let diagnostics = check(source, "generic_class_method_assigns_this_property.ts");
    single_error(&diagnostics, DiagnosticCode::ArgumentNotAssignable);
}

// `extends Box<number>`: the parent's own declared type parameter is bound to
// the argument given in the heritage clause, the same way a type reference
// like Box<number> substitutes elsewhere. Previously the heritage clause only
// captured the parent's name, never its type arguments, so an inherited
// generic base kept its bare, unsubstituted shape.
#[test]
fn generic_base_class_field_is_substituted() {
    let source =
        include_str!("fixtures/generic-classes/generic_base_class_field_is_substituted.ts");
    let diagnostics = check(source, "generic_base_class_field_is_substituted.ts");
    single_error(&diagnostics, DiagnosticCode::ReturnTypeMismatch);
}

// An explicit type argument on `new` locks T the same way it does for a
// generic function call, overriding what the constructor argument would
// otherwise have inferred.
#[test]
fn explicit_type_argument_on_new_overrides_inference() {
    let source = include_str!(
        "fixtures/generic-classes/explicit_type_argument_on_new_overrides_inference.ts"
    );
    let diagnostics = check(
        source,
        "explicit_type_argument_on_new_overrides_inference.ts",
    );
    single_error(&diagnostics, DiagnosticCode::ArgumentNotAssignable);
}
