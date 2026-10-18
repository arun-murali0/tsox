use tsox::{DiagnosticCode, TypeChecker};

// Reading a property on a union follows tsc: it is allowed when every member has
// the property, and its type is the union of the members' property types. The
// discriminated-union narrowing tests depend on this, because `shape.kind` is read
// from the un-narrowed union before any narrowing happens.

fn check(source: &str) -> Vec<tsox::Diagnostic> {
    let checked = TypeChecker::new().check_source(source, "union_member_access.ts");
    assert!(checked.is_ok(), "source should at least parse: {checked:?}");
    checked
        .map(|checked| checked.diagnostics)
        .unwrap_or_default()
}

#[test]
fn a_property_every_member_has_is_readable() {
    let diagnostics = check(
        "type A = { id: number; a: string } | { id: number; b: string };\n\
         function f(x: A): number { return x.id; }\n",
    );
    assert!(diagnostics.is_empty(), "got: {diagnostics:?}");
}

#[test]
fn the_result_is_the_union_of_the_members_property_types() {
    let diagnostics =
        check("function f(x: { v: number } | { v: string }): number | string { return x.v; }\n");
    assert!(diagnostics.is_empty(), "got: {diagnostics:?}");
}

#[test]
fn a_member_without_the_property_is_reported_once() {
    let diagnostics = check(
        "function f(x: { id: number; a: string } | { id: number }): string { return x.a; }\n",
    );
    assert_eq!(diagnostics.len(), 1, "got: {diagnostics:?}");
    assert_eq!(diagnostics[0].code, DiagnosticCode::PropertyDoesNotExist);
}
