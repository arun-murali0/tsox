use tsox::TypeChecker;

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
fn literal_union_annotation_accepts_a_listed_value() {
    let source = include_str!("fixtures/literal-widening/literal_union_annotation.ts");
    let diagnostics = check(source, "literal_union_annotation.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn literal_union_rejects_a_value_not_in_the_union() {
    let source = include_str!("fixtures/literal-widening/literal_union_rejects_unlisted_value.ts");
    let diagnostics = check(source, "literal_union_rejects_unlisted_value.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
}

#[test]
fn let_widens_to_primitive_and_stays_referenceable() {
    let source = include_str!("fixtures/literal-widening/let_widens_to_primitive.ts");
    let diagnostics = check(source, "let_widens_to_primitive.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn const_keeps_its_literal_type() {
    let source = include_str!("fixtures/literal-widening/const_keeps_literal_type.ts");
    let diagnostics = check(source, "const_keeps_literal_type.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn let_does_not_keep_its_literal_type() {
    let source = include_str!("fixtures/literal-widening/let_does_not_keep_literal_type.ts");
    let diagnostics = check(source, "let_does_not_keep_literal_type.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
}

#[test]
fn array_literal_elements_widen_before_forming_the_element_type() {
    let source = include_str!("fixtures/literal-widening/array_literal_elements_widen.ts");
    let diagnostics = check(source, "array_literal_elements_widen.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}
