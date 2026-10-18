use tsox::{FileId, Severity, UnitContext, Worker};

// A `namespace N { ... }` is a scope for the names written in it, a place its exported
// types live (`N.I`), and, when every exported value in it has a type, an object value.

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
fn an_exported_function_of_a_namespace_is_a_property_of_its_value() {
    let source = "namespace N { export function f(n: number): number { return n } }
        const a: number = N.f(1);
        const b: string = N.f(1);";
    let found = errors(source);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, Some(2322));
}

#[test]
fn an_exported_type_of_a_namespace_is_named_with_a_dotted_path() {
    let source = "namespace N { export interface P { x: number } }
        const ok: N.P = { x: 1 };
        const bad: N.P = { x: 'a' };";
    let found = errors(source);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, Some(2322));
}

#[test]
fn a_sibling_type_written_without_the_prefix_is_found_inside_the_namespace() {
    let source = "namespace N {
            export interface P { x: number }
            export function make(): P { return { x: 1 } }
            export function bad(): P { return { x: 'a' } }
        }
        const p: N.P = N.make();";
    let found = errors(source);
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn nested_namespaces_resolve_outward() {
    let source = "namespace A {
            export interface T { a: number }
            export namespace B {
                export function f(t: T): number { return t.a }
            }
        }
        const n: string = A.B.f({ a: 1 });";
    let found = errors(source);
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn a_name_that_is_not_exported_is_not_a_property() {
    let source =
        "namespace N { function hidden(): number { return 1 } export const x: number = 1; }
        N.hidden;";
    let found = errors(source);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, Some(2339));
}

#[test]
fn an_ambient_namespace_exports_every_member() {
    let source = "declare namespace Lib { function parse(s: string): number; const VERSION: string; interface Opts { strict: boolean } }
        const n: number = Lib.parse('1');
        const v: number = Lib.VERSION;
        const o: Lib.Opts = { strict: 1 };";
    let found = errors(source);
    assert_eq!(found.len(), 2, "{found:?}");
}

#[test]
fn a_namespace_with_an_unannotated_exported_variable_stays_unknown_not_wrong() {
    // `x` is typed from its initializer only after the body is checked, so the object is
    // not built: using the namespace must not claim a property is missing.
    let source = "namespace N { export const x = 1; export function f(): number { return x } }
        N.x;
        N.f();";
    assert!(errors(source).is_empty(), "{:?}", errors(source));
}

#[test]
fn a_namespace_merged_with_a_function_keeps_both_parts() {
    // `f()` still calls, and `f.extra` is the namespace's member. Both are typed, so a
    // wrong use of either is reported and a right one is not.
    let source = "function f(): number { return 1 }
        namespace f { export const extra: string = 'e'; }
        const a: number = f();
        const b: string = f.extra;
        const wrong_call: string = f();
        const wrong_member: number = f.extra;";
    let found = errors(source);
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(
        found.iter().all(|(code, _)| *code == Some(2322)),
        "{found:?}"
    );
}

#[test]
fn the_body_of_a_function_merged_with_a_namespace_is_still_checked() {
    // The merged value is an intersection, and the body is checked against the signature
    // half of it. Reading the symbol as a plain function would skip the body silently.
    let source = "function f(n: number): number { const s: string = n; return n }
        namespace f { export const extra: number = 2; }";
    let found = errors(source);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, Some(2322));
}

#[test]
fn a_namespace_written_before_its_function_merges_the_same_way() {
    let source = "namespace f { export const extra: string = 'e'; }
        function f(): number { return 1 }
        const a: number = f();
        const b: number = f.extra;";
    let found = errors(source);
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn a_namespace_merged_with_a_class_keeps_the_constructor_and_the_members() {
    let source = "class C { v: number = 1 }
        namespace C { export const make: () => C = () => new C(); export const tag: string = 't'; }
        const a: C = new C();
        const b: C = C.make();
        const c: string = C.tag;
        const wrong_instance: number = new C();
        const wrong_member: number = C.tag;";
    let found = errors(source);
    assert_eq!(found.len(), 2, "{found:?}");
}

#[test]
fn a_namespace_merged_with_an_enum_keeps_the_members_of_both() {
    let source = "enum E { X, Y }
        namespace E { export const first: E = E.X; }
        const a: E = E.first;
        const b: E = E.Y;
        const wrong: string = E.first;";
    let found = errors(source);
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn a_namespace_sharing_a_name_with_an_untyped_function_is_left_alone() {
    // Nothing to put the object next to, so nothing is claimed about either.
    let source = "function f(n) { return n }
        namespace f { export const extra: string = 'e'; }
        f(1);
        f.extra;";
    let found = errors(source);
    assert!(
        found.iter().all(|(code, _)| *code != Some(2339)),
        "{found:?}"
    );
}

#[test]
fn two_blocks_of_the_same_namespace_are_one_object() {
    let source = "namespace N { export const a: number = 1; }
        namespace N { export const b: string = 'b'; }
        const x: number = N.a;
        const y: string = N.b;
        const wrong: number = N.b;
        N.missing;";
    let found = errors(source);
    assert_eq!(found.len(), 2, "{found:?}");
    assert_eq!(found[0].0, Some(2322));
    assert_eq!(found[1].0, Some(2339));
}

#[test]
fn an_enum_and_a_class_inside_a_namespace_are_values_of_it() {
    let source = "namespace N {
            export enum Color { Red }
            export class K { n: number = 1 }
        }
        const c: string = N.Color.Red;
        const k: string = N.K;";
    let found = errors(source);
    assert_eq!(found.len(), 2, "{found:?}");
}

#[test]
fn an_error_inside_a_namespace_body_is_reported() {
    let found = errors("namespace N { export const x: number = 'a'; }");
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn a_dotted_namespace_name_is_a_namespace_inside_a_namespace() {
    let source = "namespace A.B { export const x: number = 1; export interface T { n: number } }
        const a: number = A.B.x;
        const bad: string = A.B.x;
        const t: A.B.T = { n: 1 };
        const bad_t: A.B.T = { n: 's' };";
    let found = errors(source);
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(
        found.iter().all(|(code, _)| *code == Some(2322)),
        "{found:?}"
    );
}

#[test]
fn a_dotted_name_with_three_parts_nests_three_deep() {
    let source = "namespace A.B.C { export function f(): number { return 1 } }
        const a: number = A.B.C.f();
        const bad: string = A.B.C.f();";
    let found = errors(source);
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn the_names_around_a_dotted_namespace_are_found_without_the_prefix() {
    // B's body is checked with A as an enclosing scope, so A's own members are in reach,
    // and errors inside it are reported like in any other namespace body.
    let source = "namespace A {
            export interface Shared { n: number }
        }
        namespace A.B {
            export function f(s: Shared): number { return s.n }
            export const wrong: number = 'a';
        }
        const r: number = A.B.f({ n: 1 });";
    let found = errors(source);
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn a_dotted_namespace_and_a_nested_one_of_the_same_path_are_one_object() {
    let source = "namespace A.B { export const x: number = 1; }
        namespace A { export namespace B { export const y: string = 'y'; } }
        const a: number = A.B.x;
        const b: string = A.B.y;";
    assert!(errors(source).is_empty(), "{:?}", errors(source));
}

// `new` on a member: a class that lives inside a namespace.
#[test]
fn new_on_a_namespace_member_is_checked_like_new_on_a_name() {
    let source = "namespace N { export class K { constructor(public v: number) {} } }
        const ok: N.K = new N.K(1);
        const wrong_instance: string = new N.K(1);
        const wrong_argument = new N.K('a');
        const wrong_count = new N.K();";
    let found = errors(source);
    assert_eq!(found.len(), 3, "{found:?}");
    let codes: Vec<_> = found.iter().map(|(code, _)| *code).collect();
    assert_eq!(codes, vec![Some(2322), Some(2345), Some(2554)], "{found:?}");
}

#[test]
fn new_on_a_class_in_a_dotted_namespace_is_checked() {
    let source = "namespace A.B { export class K { v: number = 1 } }
        const ok: A.B.K = new A.B.K();
        const wrong: string = new A.B.K();";
    let found = errors(source);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, Some(2322));
}

#[test]
fn new_on_a_generic_class_in_a_namespace_infers_its_type_argument() {
    let source = "namespace N { export class Box<T> { constructor(public value: T) {} } }
        const b = new N.Box(1);
        const n: number = b.value;
        const wrong: string = b.value;";
    let found = errors(source);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, Some(2322));
}

#[test]
fn new_on_a_member_that_is_not_a_class_is_reported() {
    let source = "namespace N { export const x: number = 1; }
        new N.x();";
    let found = errors(source);
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn new_on_a_member_of_something_with_no_such_member_is_reported_once() {
    let source = "namespace N { export const x: number = 1; }
        new N.Missing();";
    let found = errors(source);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, Some(2339));
}

// Problem: `new Intl.DateTimeFormat()` started reporting TS2304 for `Intl` once `new`
// accepted a member expression, because the library global is not modelled. Picked: treat
// it as a known ambient global like Math and JSON, so its members are unchecked.
#[test]
fn new_on_an_unmodelled_library_namespace_is_not_reported() {
    let found = errors("const f = new Intl.DateTimeFormat();\n");
    assert!(found.is_empty(), "{found:?}");
}

// A class left unexported in a second block of a namespace is local to that block; it is
// not a second declaration of the exported one (tsc accepts this, and reports no TS2300).
#[test]
fn an_unexported_class_in_a_later_block_does_not_collide_with_an_exported_one() {
    let found = errors(
        "namespace Z { export class Line { length!: number } }\n\
         namespace Z { class Line { name!: string } }\n\
         var l: { length: number };\n\
         var l: Z.Line;\n",
    );
    assert!(
        found.iter().all(|(code, _)| *code != Some(2300)),
        "{found:?}"
    );
}

// Inside a namespace, exported declarations are one symbol across blocks, so two exported
// classes of one name are a redeclaration; two local classes in one block are too. A local
// class in another block is a different symbol.
#[test]
fn exported_classes_with_one_name_in_two_blocks_are_a_duplicate() {
    let found = errors("namespace N { export class C {} }\nnamespace N { export class C {} }\n");
    assert!(
        found.iter().any(|(code, _)| *code == Some(2300)),
        "{found:?}"
    );
}

#[test]
fn local_classes_with_one_name_in_one_block_are_a_duplicate() {
    let found = errors("namespace N { class C {} class C {} }\n");
    assert!(
        found.iter().any(|(code, _)| *code == Some(2300)),
        "{found:?}"
    );
}

#[test]
fn local_classes_with_one_name_in_different_blocks_are_not() {
    let found = errors("namespace N { class C {} }\nnamespace N { class C {} }\n");
    assert!(
        found.iter().all(|(code, _)| *code != Some(2300)),
        "{found:?}"
    );
}

#[test]
fn common_library_globals_are_not_reported_as_unknown_names() {
    let found = errors(
        "const a = new Uint8Array(4);\nconst u = new URL('x');\nconst d = document.title;\nconst e = encodeURIComponent('x');\nconst r = fetch('x');\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

// A class exported from a namespace that is not itself exported lives in that local
// namespace, so it is not a second declaration of the class in the exported namespace.
#[test]
fn an_exported_class_inside_a_local_namespace_does_not_collide_with_the_exported_one() {
    let found = errors(
        "namespace X.Y.Z { export class Line { length!: number } }\n\
         namespace X { export namespace Y { namespace Z { export class Line { name!: string } } } }\n",
    );
    assert!(
        found.iter().all(|(code, _)| *code != Some(2300)),
        "{found:?}"
    );
}

// Inside a namespace a member is named by its short name in a message, as tsc prints it; from
// outside, the qualified name stays.
#[test]
fn messages_inside_a_namespace_use_the_short_member_name() {
    let found = errors(
        "namespace Test { interface T { a: number } export function f(): T { return null as any } \
         export const x: T = 5; }\n",
    );
    assert!(
        found
            .iter()
            .any(|(_, m)| m.contains("to type 'T'") && !m.contains("Test.T")),
        "{found:?}"
    );
}

#[test]
fn messages_outside_a_namespace_keep_the_qualified_name() {
    let found =
        errors("namespace Test { export interface T { a: number } }\nconst x: Test.T = 5;\n");
    assert!(found.iter().any(|(_, m)| m.contains("Test.T")), "{found:?}");
}
