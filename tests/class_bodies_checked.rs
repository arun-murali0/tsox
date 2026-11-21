use tsox::{FileId, Severity, UnitContext, Worker};

// Bodies of a class that were skipped: accessors, instance property initializers, a
// constructor with an unresolved parameter. Also overloaded methods and a narrowed `this._x`.

fn error_lines(source: &str) -> Vec<usize> {
    let ctx = UnitContext::new(FileId::ROOT, "a.ts", source);
    Worker::new()
        .check_unit(&ctx)
        .unwrap()
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| source[..d.start as usize].matches('\n').count() + 1)
        .collect()
}

#[test]
fn instance_accessor_bodies_are_checked() {
    let source = "class A {\n  get g(): number { return \"s\"; }\n  set s(v: number) { const q: string = v; }\n}";
    assert_eq!(error_lines(source), [2, 3]);
}

#[test]
fn static_accessor_bodies_are_checked() {
    assert_eq!(
        error_lines("class A {\n  static get g(): number { return \"s\"; }\n}"),
        [2]
    );
}

#[test]
fn an_instance_property_initializer_is_checked_against_its_annotation() {
    assert_eq!(
        error_lines("class A {\n  x: number = \"s\";\n  y: string = \"ok\";\n}"),
        [2]
    );
}

#[test]
fn a_constructor_body_is_checked_when_a_parameter_does_not_resolve() {
    let lines =
        error_lines("class A {\n  constructor(p: Missing) {\n    const z: string = 1;\n  }\n}");
    assert!(lines.contains(&3), "{lines:?}");
}

#[test]
fn overloaded_methods_take_their_signatures_not_the_implementation() {
    let source = "class A {\n  p(a: number): number;\n  p(a: number, b: true): number;\n  p(a: number) { return a; }\n  m() { this.p(1, true); this.p(1); }\n}";
    assert!(error_lines(source).is_empty());
}

#[test]
fn a_member_read_through_this_is_narrowed() {
    let source = "type Foo = { bar: number };\nclass C {\n  _x: number | Foo = 0;\n  get x(): number { return typeof this._x === 'number' ? this._x : this._x.bar; }\n}";
    assert!(error_lines(source).is_empty());
}

#[test]
fn a_narrowed_member_is_forgotten_after_it_is_assigned() {
    let source = "class C {\n  _x: number | string = 0;\n  m() {\n    if (typeof this._x === 'number') {\n      this._x = \"s\";\n      return this._x.toFixed();\n    }\n    return \"\";\n  }\n}";
    assert_eq!(error_lines(source), [6]);
}

#[test]
fn a_narrowed_property_path_is_forgotten_when_its_root_is_assigned() {
    let source = "declare var o: { a: string | number };\nif (typeof o.a === \"string\") {\n  o = { a: 1 };\n  o.a.toUpperCase();\n}";
    assert_eq!(error_lines(source), [4]);
}
