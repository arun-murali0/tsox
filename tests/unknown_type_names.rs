use tsox::{DiagnosticCode, Severity, TypeChecker};

// Problem: an unknown name in a type annotation was reported only where a variable or a
// parameter happened to carry it. In an alias body, an interface member, a return type, a
// class property, a type argument or one side of a union it passed in silence, where tsc
// says "Cannot find name 'Missing'." (TS2304) at the name.
// Now: one pass over every type reference in the file reports each unknown name once.
// The names the standard library provides (`Date`, `Map`, `HTMLElement`, ...), type
// parameters and types declared inside a function are not unknown.
//
// Only errors are compared: tsox also says, as a warning, that an annotation could not be
// resolved when something uses an alias whose body holds an unknown name. tsc prints no such
// line, and scripts/ts-diag-tool/compare.js counts errors, so those are left out here too.

fn errors(source: &str, file_name: &str) -> Vec<(DiagnosticCode, String)> {
    let result = TypeChecker::new().check_source(source, file_name);
    assert!(result.is_ok(), "fixture should at least parse: {result:?}");
    result
        .map(|checked| checked.diagnostics)
        .unwrap_or_default()
        .into_iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .map(|diagnostic| (diagnostic.code, diagnostic.message))
        .collect()
}

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!("fixtures/unknown-type-names/", $name, ".ts"))
    };
}

fn cannot_find(name: &str) -> (DiagnosticCode, String) {
    (
        DiagnosticCode::UnresolvedIdentifier,
        format!("Cannot find name '{name}'."),
    )
}

#[test]
fn an_alias_of_an_intersection_names_the_unknown() {
    assert_eq!(
        errors(
            fixture!("alias_intersection_names_the_unknown"),
            "alias_intersection_names_the_unknown.ts"
        ),
        vec![cannot_find("Missing")]
    );
}

#[test]
fn an_alias_of_a_union_names_the_unknown() {
    assert_eq!(
        errors(
            fixture!("alias_union_names_the_unknown"),
            "alias_union_names_the_unknown.ts"
        ),
        vec![cannot_find("Missing")]
    );
}

#[test]
fn an_interface_member_names_the_unknown() {
    assert_eq!(
        errors(
            fixture!("interface_member_names_the_unknown"),
            "interface_member_names_the_unknown.ts"
        ),
        vec![cannot_find("Missing")]
    );
}

#[test]
fn a_return_type_names_the_unknown() {
    assert_eq!(
        errors(
            fixture!("return_type_names_the_unknown"),
            "return_type_names_the_unknown.ts"
        ),
        vec![cannot_find("Missing")]
    );
}

#[test]
fn one_side_of_a_parameter_union_names_the_unknown() {
    assert_eq!(
        errors(
            fixture!("parameter_union_names_the_unknown"),
            "parameter_union_names_the_unknown.ts"
        ),
        vec![cannot_find("Missing")]
    );
}

#[test]
fn a_class_property_names_the_unknown() {
    assert_eq!(
        errors(
            fixture!("class_property_names_the_unknown"),
            "class_property_names_the_unknown.ts"
        ),
        vec![cannot_find("Missing")]
    );
}

#[test]
fn a_type_argument_names_the_unknown() {
    assert_eq!(
        errors(
            fixture!("generic_argument_names_the_unknown"),
            "generic_argument_names_the_unknown.ts"
        ),
        vec![cannot_find("Missing")]
    );
}

#[test]
fn two_unknown_names_are_two_errors() {
    assert_eq!(
        errors(
            fixture!("two_unknowns_are_two_errors"),
            "two_unknowns_are_two_errors.ts"
        ),
        vec![cannot_find("Missing"), cannot_find("Other")]
    );
}

#[test]
fn library_type_parameter_and_local_names_are_not_reported() {
    let source = fixture!("library_and_declared_names_are_not_reported");
    let result = TypeChecker::new().check_source(source, "library_and_declared_names.ts");
    let diagnostics = result.map(|c| c.diagnostics).unwrap_or_default();
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.severity != Severity::Error),
        "expected no errors, got: {diagnostics:?}"
    );
}
