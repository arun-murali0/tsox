use tsox::{DiagnosticCode, TypeChecker};

// Duplicate type-space declarations. The pairs TypeScript rejects report one
// duplicate-declaration error, and the pairs it merges report nothing.

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!("fixtures/declaration-collisions/", $name))
    };
}

fn check(source: &str, file_name: &str) -> Vec<tsox::Diagnostic> {
    let result = TypeChecker::new().check_source(source, file_name);
    assert!(result.is_ok(), "fixture should at least parse: {result:?}");
    result
        .map(|checked| checked.diagnostics)
        .unwrap_or_default()
}

fn assert_clean(file_name: &str, source: &str) {
    let diagnostics = check(source, file_name);
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

// tsc reports a duplicate identifier on every declaration involved, so a collision of
// two declarations is two diagnostics, one at each, at different positions.
fn assert_both_declarations_flagged(file_name: &str, source: &str, code: DiagnosticCode) {
    let diagnostics = check(source, file_name);
    assert_eq!(
        diagnostics.len(),
        2,
        "expected one diagnostic per declaration, got: {diagnostics:?}"
    );
    assert!(
        diagnostics.iter().all(|d| d.code == code),
        "{diagnostics:?}"
    );
    assert_ne!(
        diagnostics[0].start, diagnostics[1].start,
        "{diagnostics:?}"
    );
}

#[test]
fn interfaces_with_the_same_name_merge() {
    assert_clean(
        "interfaces_with_the_same_name_merge.ts",
        fixture!("interfaces_with_the_same_name_merge.ts"),
    );
}

#[test]
fn an_interface_and_a_class_with_the_same_name_merge() {
    assert_clean(
        "an_interface_and_a_class_with_the_same_name_merge.ts",
        fixture!("an_interface_and_a_class_with_the_same_name_merge.ts"),
    );
}

#[test]
fn distinct_names_do_not_collide() {
    assert_clean(
        "distinct_names_do_not_collide.ts",
        fixture!("distinct_names_do_not_collide.ts"),
    );
}

#[test]
fn duplicate_type_alias_is_an_error() {
    assert_both_declarations_flagged(
        "duplicate_type_alias_is_an_error.ts",
        fixture!("duplicate_type_alias_is_an_error.ts"),
        DiagnosticCode::DuplicateTypeDeclaration,
    );
}

#[test]
fn type_alias_and_class_with_the_same_name_is_an_error() {
    assert_both_declarations_flagged(
        "type_alias_and_class_with_the_same_name_is_an_error.ts",
        fixture!("type_alias_and_class_with_the_same_name_is_an_error.ts"),
        DiagnosticCode::DuplicateTypeDeclaration,
    );
}

#[test]
fn two_classes_with_the_same_name_is_an_error() {
    assert_both_declarations_flagged(
        "two_classes_with_the_same_name_is_an_error.ts",
        fixture!("two_classes_with_the_same_name_is_an_error.ts"),
        DiagnosticCode::DuplicateTypeDeclaration,
    );
}

#[test]
fn type_alias_and_interface_with_the_same_name_is_an_error() {
    assert_both_declarations_flagged(
        "type_alias_and_interface_with_the_same_name_is_an_error.ts",
        fixture!("type_alias_and_interface_with_the_same_name_is_an_error.ts"),
        DiagnosticCode::DuplicateTypeDeclaration,
    );
}

#[test]
fn enum_and_type_alias_with_the_same_name_is_an_error() {
    assert_both_declarations_flagged(
        "enum_and_type_alias_with_the_same_name_is_an_error.ts",
        fixture!("enum_and_type_alias_with_the_same_name_is_an_error.ts"),
        DiagnosticCode::EnumDeclarationMerge,
    );
}

// Three declarations of one name: the first is reported once, not once per collision.
#[test]
fn a_third_declaration_does_not_report_the_first_again() {
    let diagnostics = check(
        "type T = number;\ntype T = string;\ntype T = boolean;\n",
        "three.ts",
    );
    assert_eq!(diagnostics.len(), 3, "{diagnostics:?}");
    let mut starts: Vec<u32> = diagnostics.iter().map(|d| d.start).collect();
    starts.dedup();
    assert_eq!(
        starts.len(),
        3,
        "each declaration flagged once: {diagnostics:?}"
    );
}
