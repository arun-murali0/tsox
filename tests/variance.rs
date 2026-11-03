use tsox::{Severity, TypeChecker};

fn error_count(source: &str) -> usize {
    let result = TypeChecker::new().check_source(source, "variance.ts");
    let checked = result.expect("fixture should parse");
    checked
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count()
}

// Each case assigns one application of a generic interface to another and counts errors.
fn assign(decl: &str, from: &str, to: &str) -> usize {
    error_count(&format!(
        "{decl}\ndeclare const source: {from};\nconst target: {to} = source;\n"
    ))
}

const WIDE: &str = "number | string";

#[test]
fn covariant_parameter_accepts_narrower_argument_only() {
    let decl = "interface Box<T> { value: T }";
    assert_eq!(assign(decl, "Box<number>", &format!("Box<{WIDE}>")), 0);
    assert_eq!(assign(decl, &format!("Box<{WIDE}>"), "Box<number>"), 1);
}

#[test]
fn contravariant_parameter_accepts_wider_argument_only() {
    let decl = "interface Sink<T> { put: (x: T) => void }";
    assert_eq!(assign(decl, &format!("Sink<{WIDE}>"), "Sink<number>"), 0);
    assert_eq!(assign(decl, "Sink<number>", &format!("Sink<{WIDE}>")), 1);
}

#[test]
fn method_parameter_is_bivariant() {
    let decl = "interface Bag<T> { put(x: T): void }";
    assert_eq!(assign(decl, "Bag<number>", &format!("Bag<{WIDE}>")), 0);
    assert_eq!(assign(decl, &format!("Bag<{WIDE}>"), "Bag<number>"), 0);
}

#[test]
fn read_and_write_use_is_invariant() {
    let decl = "interface Cell<T> { get: () => T; set: (x: T) => void }";
    assert_eq!(assign(decl, "Cell<number>", &format!("Cell<{WIDE}>")), 1);
    assert_eq!(assign(decl, &format!("Cell<{WIDE}>"), "Cell<number>"), 1);
    assert_eq!(assign(decl, "Cell<number>", "Cell<number>"), 0);
}

#[test]
fn unused_parameter_is_independent() {
    let decl = "interface Tag<T> { id: number }";
    assert_eq!(assign(decl, "Tag<string>", "Tag<number>"), 0);
}

#[test]
fn self_recursive_declaration_keeps_its_variance() {
    let decl = "interface Node<T> { value: T; next: Node<T> | null }";
    assert_eq!(assign(decl, "Node<number>", &format!("Node<{WIDE}>")), 0);
    assert_eq!(assign(decl, &format!("Node<{WIDE}>"), "Node<number>"), 1);
}

#[test]
fn self_application_with_a_changed_argument_is_covariant_here() {
    let decl = "interface Vec<A> { x: A; apply<B>(f: Vec<(a: A) => B>): Vec<B> }";
    assert_eq!(assign(decl, "Vec<number>", &format!("Vec<{WIDE}>")), 0);
}

#[test]
fn variance_composes_through_another_declaration() {
    let decl = "interface Sink<T> { put: (x: T) => void }\ninterface Holder<T> { sink: Sink<T> }";
    assert_eq!(assign(decl, &format!("Holder<{WIDE}>"), "Holder<number>"), 0);
    assert_eq!(assign(decl, "Holder<number>", &format!("Holder<{WIDE}>")), 1);
}

#[test]
fn constructor_result_is_related_to_the_class_through_variance() {
    let source = "class Vec2<A> { constructor(public x: A, public y: A) {} }
const made: Vec2<number> = new Vec2(1, 2);
const wide: Vec2<number | string> = new Vec2(1, 2);\n";
    assert_eq!(error_count(source), 0);
}

// tsc reports nothing for this. The class's own method type parameter `B` was not active
// while the body was checked, so `f.x(this.x)` read B as unbound and gave `unknown`.
#[test]
fn self_applying_class_checks_without_error() {
    let source = "class Vec2<A> { constructor(public x: A, public y: A) {}
  apply<B>(f: Vec2<(a: A) => B>): Vec2<B> { return new Vec2(f.x(this.x), f.y(this.y)); } }\n";
    assert_eq!(error_count(source), 0);
}

#[test]
fn a_method_body_sees_its_own_type_parameter() {
    let source = "class W<A> { constructor(public x: A) {}
  m<B>(f: (a: A) => B): B { return f(this.x); }
  static s<T>(f: () => T): T { return f(); } }\n";
    assert_eq!(error_count(source), 0);
}
