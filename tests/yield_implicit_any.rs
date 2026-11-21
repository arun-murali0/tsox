use tsox::{FileId, Severity, UnitContext, Worker};

// TS7057: a `yield` whose result is read, in a generator that writes no return type, and
// that has no contextual type to take its type from.

fn codes(source: &str) -> Vec<Option<u32>> {
    let ctx = UnitContext::new(FileId::ROOT, "a.ts", source);
    Worker::new()
        .check_unit(&ctx)
        .unwrap()
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.code.tsc_code())
        .collect()
}

#[test]
fn a_result_read_into_an_unannotated_variable_is_reported() {
    assert_eq!(codes("function* g() { const v = yield; }"), [Some(7057)]);
}

#[test]
fn an_annotated_target_gives_the_yield_a_type() {
    assert!(codes("function* g() { const v: string = yield; }").is_empty());
}

#[test]
fn a_statement_leaves_the_result_unused() {
    assert!(codes("function* g() { yield; yield 1; }").is_empty());
}

#[test]
fn a_return_type_on_the_generator_gives_the_yield_a_type() {
    assert!(
        codes("function* g(): Generator<number, void, string> { const v = yield 1; }").is_empty()
    );
}

#[test]
fn a_delegating_yield_is_not_any() {
    assert!(codes("function* g() { const v = yield* [1]; }").is_empty());
}

#[test]
fn the_operand_of_a_yield_and_a_default_value_are_reported() {
    assert_eq!(codes("function* g() { yield yield 0; }"), [Some(7057)]);
    assert_eq!(codes("function* g(a = yield) {}"), [Some(7057)]);
}
