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

#[test]
fn interface_members_are_added_to_the_class_instance_type() {
    let source = "interface Pet { name: string }
        class Pet { age: number = 0 }
        const pet: Pet = new Pet();
        const petName: string = new Pet().name;
        const age: number = new Pet().age;";
    assert_clean("class_interface_members.ts", source);
}

#[test]
fn class_and_interface_merge_in_either_declaration_order() {
    let source = "class Pet { age: number = 0 }
        interface Pet { name: string }
        const pet: Pet = new Pet();
        const petName: string = new Pet().name;";
    assert_clean("class_interface_order.ts", source);
}

#[test]
fn incompatible_interface_property_merge_reports_ts2717() {
    let diagnostics = check(
        "interface A { x: number }
interface A { x: string }",
        "interface_property_conflict.ts",
    );
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].code.tsc_code(), Some(2717));
}

#[test]
fn only_one_merged_enum_declaration_may_omit_first_member_initializer() {
    let diagnostics = check("enum E { A }\nenum E { B }", "enum_merge_initializer.ts");
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].code.tsc_code(), Some(2432));
}

#[test]
fn enum_declarations_with_distinct_members_merge() {
    let source = "enum E { A = 1 }\nenum E { B = 2 }\nconst a: E = E.A;\nconst b: E = E.B;";
    assert_clean("enum_merge.ts", source);
}

#[test]
fn incompatible_class_interface_property_reports_ts2717_in_source_order() {
    let diagnostics = check(
        "interface Pet { name: string }\nclass Pet { name: number = 1 }",
        "class_interface_property_conflict.ts",
    );
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].code.tsc_code(), Some(2717));
    assert!(
        diagnostics[0].start >= 31,
        "the second declaration should be flagged: {diagnostics:?}"
    );
}

#[test]
fn class_interface_property_conflict_is_reported_when_class_comes_first() {
    let diagnostics = check(
        "class Pet { name: number = 1 }\ninterface Pet { name: string }",
        "class_interface_property_conflict_reverse.ts",
    );
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].code.tsc_code(), Some(2717));
}

#[test]
fn merged_interface_optional_mismatch_reports_ts2687_on_both_declarations() {
    let diagnostics = check(
        "interface A { x?: string }\ninterface A { x: string }",
        "interface_modifier_conflict.ts",
    );
    assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code.tsc_code() == Some(2687))
    );
}

#[test]
fn class_interface_optional_mismatch_reports_ts2687_on_both_declarations() {
    let diagnostics = check(
        "interface A { x?: string }\nclass A { x: string = '' }",
        "class_interface_modifier_conflict.ts",
    );
    assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code.tsc_code() == Some(2687))
    );
}

#[test]
fn merged_interface_method_overloads_remain_callable() {
    let source = "interface Converter { convert(value: string): number }\n        interface Converter { convert(value: number): string }\n        declare const converter: Converter;\n        const numeric: number = converter.convert('1');\n        const textual: string = converter.convert(1);";
    assert_clean("interface_method_overloads.ts", source);
}

#[test]
fn class_interface_export_mismatch_reports_ts2395_on_both_declarations() {
    let diagnostics = check(
        "export class A {}\ninterface A {}",
        "class_interface_export_mismatch.ts",
    );
    assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code.tsc_code() == Some(2395))
    );
}

#[test]
fn merged_enum_can_omit_initializer_in_one_later_declaration_if_prior_one_did_not() {
    assert_clean(
        "enum_merge_initializer_allowed.ts",
        "enum E { A = 1 }\nenum E { B }",
    );
}

#[test]
fn class_interface_optional_mismatch_is_reported_when_class_comes_first() {
    let diagnostics = check(
        "class A { x?: string }\ninterface A { x: string }",
        "class_interface_modifier_conflict_reverse.ts",
    );
    assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code.tsc_code() == Some(2687))
    );
}
