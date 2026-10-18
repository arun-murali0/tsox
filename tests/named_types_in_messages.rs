use tsox::{Diagnostic, DiagnosticCode, Severity, TypeChecker};

fn check(fixture_source: &str, file_name: &str) -> Vec<Diagnostic> {
    let checker = TypeChecker::new();
    let result = checker.check_source(fixture_source, file_name);
    assert!(result.is_ok(), "fixture should at least parse: {result:?}");
    result
        .map(|checked| checked.diagnostics)
        .unwrap_or_default()
}

fn single_error(diagnostics: &[Diagnostic], code: DiagnosticCode) -> &Diagnostic {
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert_eq!(diagnostics[0].severity, Severity::Error);
    assert_eq!(diagnostics[0].code, code, "got: {diagnostics:?}");
    &diagnostics[0]
}

// An interface prints as its own declared name on the *expected* side of a
// mismatch, not the member list its shape happens to be. Like tsc, a missing
// property is reported as TS2741 (MissingProperty); this test checks that the
// name shows up where it should.
#[test]
fn interface_prints_its_own_name() {
    let source = include_str!("fixtures/named-types/interface_prints_its_own_name.ts");
    let diagnostics = check(source, "interface_prints_its_own_name.ts");
    let error = single_error(&diagnostics, DiagnosticCode::MissingProperty);
    assert!(error.message.contains("'Dog'"), "got: {error:?}");
    assert!(
        !error.message.contains("breed: string"),
        "should print the name, not unfold the shape: got {error:?}"
    );
}

#[test]
fn type_alias_prints_its_own_name() {
    let source = include_str!("fixtures/named-types/type_alias_prints_its_own_name.ts");
    let diagnostics = check(source, "type_alias_prints_its_own_name.ts");
    let error = single_error(&diagnostics, DiagnosticCode::MissingProperty);
    assert!(error.message.contains("'Pair'"), "got: {error:?}");
}

// tsc: "Type 'Point' is not assignable to type 'string'."
#[test]
fn class_prints_its_own_name() {
    let source = include_str!("fixtures/named-types/class_prints_its_own_name.ts");
    let diagnostics = check(source, "class_prints_its_own_name.ts");
    let error = single_error(&diagnostics, DiagnosticCode::ReturnTypeMismatch);
    assert!(error.message.contains("'Point'"), "got: {error:?}");
}

// tsc: "Type 'Color' is not assignable to type 'string'."
#[test]
fn enum_prints_its_own_name() {
    let source = include_str!("fixtures/named-types/enum_prints_its_own_name.ts");
    let diagnostics = check(source, "enum_prints_its_own_name.ts");
    let error = single_error(&diagnostics, DiagnosticCode::ReturnTypeMismatch);
    assert!(error.message.contains("'Color'"), "got: {error:?}");
}

// tsc: "Type 'Box<number>' is not assignable to type 'string'."
#[test]
fn generic_instantiation_prints_with_its_argument() {
    let source =
        include_str!("fixtures/named-types/generic_instantiation_prints_with_its_argument.ts");
    let diagnostics = check(source, "generic_instantiation_prints_with_its_argument.ts");
    let error = single_error(&diagnostics, DiagnosticCode::ReturnTypeMismatch);
    assert!(error.message.contains("'Box<number>'"), "got: {error:?}");
    assert!(
        !error.message.contains("value: number"),
        "should print Box<number>, not unfold its shape: got {error:?}"
    );
}

// tsc: "Type 'Box<Dog>' is not assignable to type 'string'." The argument
// itself (Dog) has to already be named by the time Box's own name is built.
#[test]
fn generic_instantiation_argument_is_itself_named() {
    let source =
        include_str!("fixtures/named-types/generic_instantiation_argument_is_itself_named.ts");
    let diagnostics = check(source, "generic_instantiation_argument_is_itself_named.ts");
    let error = single_error(&diagnostics, DiagnosticCode::ReturnTypeMismatch);
    assert!(error.message.contains("'Box<Dog>'"), "got: {error:?}");
}

// tsc: "Type 'Box<number>' is not assignable to type 'Box<string>'." Two
// instantiations of the same generic must not collide on one shared name.
#[test]
fn two_instantiations_do_not_share_a_name() {
    let source = include_str!("fixtures/named-types/two_instantiations_do_not_share_a_name.ts");
    let diagnostics = check(source, "two_instantiations_do_not_share_a_name.ts");
    let error = single_error(&diagnostics, DiagnosticCode::DeclaredTypeMismatch);
    assert!(error.message.contains("'Box<number>'"), "got: {error:?}");
    assert!(error.message.contains("'Box<string>'"), "got: {error:?}");
}
