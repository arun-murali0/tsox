use tsox::{FileId, Severity, UnitContext, Worker};

// A property whose declared type is a literal keeps its literal when the object literal is
// written against that declared type; without a declared type it widens as before.

fn errors(source: &str) -> Vec<(Option<u32>, String)> {
    let ctx = UnitContext::new(FileId::ROOT, "a.ts", source);
    Worker::new()
        .check_unit(&ctx)
        .unwrap()
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| (d.code.tsc_code(), d.message.to_string()))
        .collect()
}

#[test]
fn a_literal_property_matches_a_literal_declared_type() {
    assert!(
        errors("interface Ev { tag: 'x'; n: number } const e: Ev = { tag: 'x', n: 1 };").is_empty()
    );
}

#[test]
fn a_wrong_literal_is_still_an_error() {
    let found = errors("interface Ev { tag: 'x'; n: number } const e: Ev = { tag: 'y', n: 1 };");
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, Some(2322));
}

#[test]
fn number_and_boolean_literals_are_kept_too() {
    assert!(
        errors("interface C { v: 1 | 2; on: true } const c: C = { v: 2, on: true };").is_empty()
    );
    assert_eq!(
        errors("interface C { v: 1 | 2 } const c: C = { v: 3 };").len(),
        1
    );
}

#[test]
fn a_return_value_is_read_against_the_return_type() {
    assert!(
        errors("interface Ev { tag: 'x' } function g(): Ev { return { tag: 'x' }; }").is_empty()
    );
    assert_eq!(
        errors("interface Ev { tag: 'x' } function g(): Ev { return { tag: 'q' }; }").len(),
        1
    );
}

#[test]
fn a_call_argument_is_read_against_its_parameter() {
    assert!(errors("interface Ev { tag: 'x' } function g(e: Ev) {} g({ tag: 'x' });").is_empty());
    assert_eq!(
        errors("interface Ev { tag: 'x' } function g(e: Ev) {} g({ tag: 'z' });").len(),
        1
    );
}

#[test]
fn an_assignment_is_read_against_the_target() {
    assert!(
        errors("interface Ev { tag: 'x' } let e: Ev = { tag: 'x' }; e = { tag: 'x' };").is_empty()
    );
}

#[test]
fn array_elements_and_nested_objects_are_read_against_their_declared_types() {
    assert!(
        errors("interface Ev { tag: 'x' } const a: Ev[] = [{ tag: 'x' }, { tag: 'x' }];")
            .is_empty()
    );
    assert!(
        errors(
            "interface In { k: 'a' } interface Out { in: In } const o: Out = { in: { k: 'a' } };"
        )
        .is_empty()
    );
    assert_eq!(
        errors(
            "interface In { k: 'a' } interface Out { in: In } const o: Out = { in: { k: 'b' } };"
        )
        .len(),
        1
    );
}

#[test]
fn a_discriminated_union_keeps_each_members_tag() {
    let source = "type S = { kind: 'c'; r: number } | { kind: 's'; w: number };
        const a: S = { kind: 'c', r: 1 };
        const b: S = { kind: 's', w: 2 };";
    assert!(errors(source).is_empty(), "{:?}", errors(source));
}

#[test]
fn a_literal_stays_widened_when_nothing_declares_it() {
    // `o` has no annotation, so its `tag` is a string, and a string is not 'x'.
    let found = errors("interface Ev { tag: 'x' } const o = { tag: 'x' }; const e: Ev = o;");
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn a_string_property_still_widens_against_string() {
    assert!(errors("interface P { name: string } const p: P = { name: 'a' };").is_empty());
    let found = errors("interface P { name: string } const p: P = { name: 1 };");
    assert_eq!(found.len(), 1);
}
