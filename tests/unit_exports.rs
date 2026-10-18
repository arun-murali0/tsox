use tsox::{DeclKey, DeclKind, FileId, ModuleKey, UnitContext, UnitOutput, Worker};

// A unit run with a module key hands back the file's annotated exports as
// portable declarations, plus a name -> DeclKey table for the importer to look names up in.

fn module() -> ModuleKey {
    ModuleKey::project("a.ts")
}

fn run(source: &str) -> UnitOutput {
    let ctx = UnitContext::new(FileId::ROOT, "a.ts", source).with_module(module());
    Worker::new().check_unit(&ctx).unwrap()
}

fn key(name: &str, kind: DeclKind) -> DeclKey {
    DeclKey::new(module(), name, kind, 0)
}

fn names(out: &UnitOutput) -> Vec<&str> {
    let mut v: Vec<&str> = out.exports.iter().map(|d| &*d.name).collect();
    v.sort_unstable();
    v
}

#[test]
fn a_unit_without_a_module_key_exports_nothing() {
    let ctx = UnitContext::new(FileId::ROOT, "a.ts", "export interface P { x: number }");
    let out = Worker::new().check_unit(&ctx).unwrap();
    assert!(out.exports.is_empty());
    assert!(out.export_names.is_empty());
    assert!(!out.incomplete);
}

#[test]
fn an_exported_interface_becomes_one_portable_declaration() {
    let out = run("export interface Point { x: number; y: number }");
    assert!(!out.incomplete, "{out:?}");
    assert_eq!(out.exports.len(), 1);
    let decl = &out.exports[0];
    assert_eq!(&*decl.name, "Point");
    assert_eq!(decl.kind, DeclKind::Interface);
    assert_eq!(decl.key, key("Point", DeclKind::Interface));
    assert_eq!(
        out.export_names,
        vec![("Point".to_string(), decl.key)],
        "the table maps the name to the declaration's key"
    );
}

#[test]
fn a_declaration_that_is_not_exported_is_not_in_the_output() {
    let out = run("interface Hidden { x: number } export interface Shown { y: string }");
    assert!(!out.incomplete, "{out:?}");
    assert_eq!(names(&out), vec!["Shown"]);
}

#[test]
fn aliases_functions_and_annotated_constants_are_exported() {
    let out = run("export type Id = string;
         export type Pair = { a: number; b: string };
         export function add(a: number, b: number): number { return a + b; }
         export const limit: number = 3;");
    assert!(!out.incomplete, "{out:?}");
    assert_eq!(names(&out), vec!["Id", "Pair", "add", "limit"]);
    let kind = |n: &str| out.exports.iter().find(|d| &*d.name == n).unwrap().kind;
    assert_eq!(kind("Id"), DeclKind::Alias);
    assert_eq!(kind("Pair"), DeclKind::Alias);
    assert_eq!(kind("add"), DeclKind::Function);
    assert_eq!(kind("limit"), DeclKind::Var);
}

#[test]
fn a_generic_interface_keeps_its_type_parameters() {
    let out = run("export interface Box<T extends string> { value: T }");
    assert!(!out.incomplete, "{out:?}");
    let decl = &out.exports[0];
    assert_eq!(decl.type_params.len(), 1);
    assert_eq!(&*decl.type_params[0].name, "T");
    assert!(decl.type_params[0].bound.is_some(), "the bound is exported");
}

#[test]
fn a_generic_function_keeps_its_type_parameters() {
    let out = run("export function id<T>(x: T): T { return x; }");
    assert!(!out.incomplete, "{out:?}");
    assert_eq!(out.exports[0].type_params.len(), 1);
}

#[test]
fn a_self_referencing_interface_is_exported() {
    let out = run("export interface Node { value: number; next: Node | null }");
    assert!(!out.incomplete, "{out:?}");
    assert_eq!(out.exports.len(), 1);
}

#[test]
fn an_exported_declaration_may_name_another_exported_one() {
    let out = run("export interface User { name: string }
         export function greet(u: User): string { return u.name; }
         export const me: User = { name: 'a' };");
    assert!(!out.incomplete, "{out:?}");
    assert_eq!(names(&out), vec!["User", "greet", "me"]);
}

#[test]
fn an_export_that_names_an_unexported_declaration_exports_it_under_its_own_key() {
    // The importer is handed the reference and the declaration it points at, but only the
    // exported name can be imported.
    let out = run("interface Hidden { x: number }
         export function f(h: Hidden): number { return h.x; }");
    assert!(!out.incomplete, "{out:?}");
    let public: Vec<&str> = out.export_names.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(public, vec!["f"], "{out:?}");
    assert_eq!(names(&out), vec!["Hidden", "f"], "{out:?}");
}

#[test]
fn an_unexported_declaration_nothing_reaches_stays_in_the_file() {
    let out = run("interface Lonely { x: number }
         export interface Shown { y: number }");
    assert_eq!(names(&out), vec!["Shown"], "{out:?}");
}

// The exports are read after the bodies are checked, so what was inferred is
// what is exported.
fn body_of<'a>(out: &'a UnitOutput, name: &str) -> &'a tsox::PortableType {
    &out.exports
        .iter()
        .find(|d| &*d.name == name)
        .unwrap_or_else(|| panic!("{name} is not exported: {out:?}"))
        .body
}

#[test]
fn an_unannotated_const_is_exported_with_its_inferred_literal_type() {
    use tsox::{PNode, Prim};
    let out = run("export const n = 1; export const s = 'a';");
    assert!(!out.incomplete, "{out:?}");
    assert!(matches!(body_of(&out, "n").nodes[0], PNode::NumLit(_)));
    assert!(matches!(&body_of(&out, "s").nodes[0], PNode::StrLit(v) if &**v == "a"));
    let out = run("export let m = 1;");
    assert!(matches!(
        body_of(&out, "m").nodes[0],
        PNode::Prim(Prim::Number)
    ));
}

#[test]
fn a_function_without_a_return_annotation_is_exported_with_the_inferred_return() {
    use tsox::{PNode, Prim};
    let out = run("export function f(a: number) { return a + 1; }");
    assert!(!out.incomplete, "{out:?}");
    let body = body_of(&out, "f");
    let PNode::Function { ret, .. } = &body.nodes[body.root as usize] else {
        panic!("a function: {body:?}");
    };
    assert!(matches!(
        body.nodes[*ret as usize],
        PNode::Prim(Prim::Number)
    ));
}

#[test]
fn an_inferred_object_literal_is_exported_widened() {
    use tsox::PNode;
    let out = run("export const o = { a: 1, b: 'x' };");
    assert!(!out.incomplete, "{out:?}");
    let body = body_of(&out, "o");
    let PNode::Object { props, .. } = &body.nodes[body.root as usize] else {
        panic!("an object: {body:?}");
    };
    assert_eq!(props.len(), 2);
}

#[test]
fn an_arrow_function_constant_is_exported_with_its_inferred_signature() {
    let out = run("export const h = (a: number) => a;");
    assert!(!out.incomplete, "{out:?}");
    assert_eq!(names(&out), vec!["h"]);
}

#[test]
fn exports_do_not_change_the_diagnostics() {
    let source = "export interface P { x: number } const bad: number = 'x';";
    let with = run(source);
    let without = Worker::new()
        .check_unit(&UnitContext::new(FileId::ROOT, "a.ts", source))
        .unwrap();
    assert!(!with.diagnostics.is_empty());
    assert_eq!(with.diagnostics.len(), without.diagnostics.len());
}

#[test]
fn the_same_source_gives_the_same_keys_and_signatures_in_any_worker() {
    let source = "export interface Node { next: Node | null } export function f(n: Node): Node { return n; }";
    let one = run(source);
    let mut worker = Worker::new();
    // Warm the worker's arena with something unrelated so slot numbers differ.
    let _ = worker.check_unit(&UnitContext::new(
        FileId::ROOT,
        "z.ts",
        "interface Z { z: number } const z: Z = { z: 1 };",
    ));
    let two = worker
        .check_unit(&UnitContext::new(FileId::ROOT, "a.ts", source).with_module(module()))
        .unwrap();
    let ids = |o: &UnitOutput| {
        let mut v: Vec<_> = o.exports.iter().map(|d| (d.key, d.sig)).collect();
        v.sort();
        v
    };
    assert_eq!(ids(&one), ids(&two));
}

#[test]
fn exported_overloads_preserve_all_public_signatures_and_omit_the_implementation() {
    let out = run("export function f(a: number): number;
         export function f(a: string): string;
         export function f(a: any): any { return a; }");
    assert!(!out.incomplete, "{out:?}");
    let exported = out
        .export_names
        .iter()
        .filter(|(name, _)| name == "f")
        .count();
    assert_eq!(
        exported, 2,
        "each overload has a stable declaration key: {out:?}"
    );
    assert_eq!(
        out.exports.iter().filter(|decl| &*decl.name == "f").count(),
        2
    );
}

#[test]
fn a_function_body_refers_to_the_exported_interface_by_its_key() {
    use tsox::PNode;
    let out = run("export interface User { name: string }
         export function greet(u: User): string { return u.name; }");
    assert!(!out.incomplete, "{out:?}");
    let greet = out.exports.iter().find(|d| &*d.name == "greet").unwrap();
    let user = key("User", DeclKind::Interface);
    assert!(
        greet.body.nodes.iter().any(
            |n| matches!(n, PNode::DeclRef { key, name } if *key == user && &**name == "User")
        ),
        "{greet:?}"
    );
}

#[test]
fn a_type_parameter_is_keyed_to_the_declaration_that_declares_it() {
    use tsox::PNode;
    let out = run("export interface Box<T> { value: T }");
    let decl = &out.exports[0];
    assert!(
        decl.body
            .nodes
            .iter()
            .any(|n| matches!(n, PNode::TypeParam { key, index: 0, .. } if *key == decl.key)),
        "{decl:?}"
    );
}

#[test]
fn builtin_generics_in_a_signature_are_exportable() {
    let out = run("export function load(): Promise<number> { return null as any; }");
    assert!(!out.incomplete, "{out:?}");
    assert_eq!(names(&out), vec!["load"]);
}

#[test]
fn an_alias_to_an_exported_interface_exports() {
    let out = run("export interface U { n: number } export type V = U; export type W = V[];");
    assert!(!out.incomplete, "{out:?}");
    assert_eq!(names(&out), vec!["U", "V", "W"]);
}

#[test]
fn a_type_parameter_default_is_exported() {
    let out = run("export interface Box<T = string> { value: T }");
    assert!(!out.incomplete, "{out:?}");
    let params = &out.exports[0].type_params;
    assert_eq!(params.len(), 1);
    assert!(params[0].default.is_some(), "{params:?}");
}

#[test]
fn a_default_may_name_an_earlier_parameter() {
    let out = run("export type Pair<A, B = A> = { first: A; second: B };");
    assert!(!out.incomplete, "{out:?}");
    let params = &out.exports[0].type_params;
    assert!(params[0].default.is_none());
    assert!(params[1].default.is_some());
}

#[test]
fn editing_a_default_changes_the_signature() {
    let sig = |source: &str| run(source).exports[0].sig;
    let string = sig("export interface Box<T = string> { value: T }");
    let number = sig("export interface Box<T = number> { value: T }");
    let none = sig("export interface Box<T> { value: T }");
    assert_ne!(string, number);
    assert_ne!(string, none);
    assert_eq!(
        string,
        sig("export interface Box<T = string> { value: T }"),
        "an unchanged declaration keeps its signature"
    );
}

// -- default exports and `export { a }` ---------------------------------------------

fn table(out: &UnitOutput) -> Vec<(&str, DeclKey)> {
    let mut v: Vec<(&str, DeclKey)> = out
        .export_names
        .iter()
        .map(|(n, k)| (n.as_str(), *k))
        .collect();
    v.sort();
    v
}

#[test]
fn a_default_exported_function_is_listed_under_default() {
    let out = run("export default function area(w: number, h: number): number { return w * h; }");
    assert!(!out.incomplete, "{out:?}");
    assert_eq!(out.exports.len(), 1);
    assert_eq!(
        &*out.exports[0].name, "area",
        "the declaration keeps its name"
    );
    assert_eq!(
        table(&out),
        vec![("default", key("area", DeclKind::Function))]
    );
}

#[test]
fn named_default_function_exports_the_inferred_body_return_type() {
    use tsox::{PNode, Prim};
    let out = run("export default function render(value: number) { return value + 1; }");
    assert!(!out.incomplete, "{out:?}");
    assert!(
        out.export_names.iter().any(|(name, key)| {
            name == "default"
                && out
                    .exports
                    .iter()
                    .any(|decl| decl.key == *key && &*decl.name == "render")
        }),
        "the named function must be exported as default: {out:?}"
    );
    let body = body_of(&out, "render");
    let PNode::Function { ret, .. } = &body.nodes[body.root as usize] else {
        panic!("expected a function signature: {body:?}");
    };
    assert!(
        matches!(body.nodes[*ret as usize], PNode::Prim(Prim::Number)),
        "return type must be inferred from the body: {body:?}"
    );
}

#[test]
fn a_default_exported_interface_is_listed_under_default() {
    let out = run("export default interface Config { debug: boolean }");
    assert!(!out.incomplete, "{out:?}");
    assert_eq!(
        table(&out),
        vec![("default", key("Config", DeclKind::Interface))]
    );
}

#[test]
fn anonymous_default_class_exports_instance_and_constructor_sides() {
    let out =
        run("export default class { value: number = 1; read(): number { return this.value; } }");
    assert!(!out.incomplete, "{out:?}");
    assert_eq!(
        out.export_names
            .iter()
            .filter(|(name, _)| name == "default")
            .count(),
        2,
        "class exports type and value sides"
    );
    assert!(
        out.exports
            .iter()
            .any(|decl| decl.kind == DeclKind::Class && &*decl.name == "default")
    );
    assert!(
        out.exports
            .iter()
            .any(|decl| decl.kind == DeclKind::Var && &*decl.name == "default")
    );
}

#[test]
fn default_expression_exports_its_inferred_type() {
    use tsox::PNode;
    let out = run("export default 1 + 2;");
    assert!(!out.incomplete, "{out:?}");
    assert!(out.export_names.iter().any(|(name, _)| name == "default"));
    let body = body_of(&out, "default");
    assert!(matches!(
        body.nodes[body.root as usize],
        PNode::Prim(tsox::Prim::Number)
    ));
}

#[test]
fn anonymous_default_function_exports_inferred_return_type() {
    use tsox::{PNode, Prim};
    let out = run("export default function (a: number) { return a + 1; }");
    assert!(!out.incomplete, "{out:?}");
    let body = body_of(&out, "default");
    let PNode::Function { ret, .. } = &body.nodes[body.root as usize] else {
        panic!("a function: {body:?}");
    };
    assert!(
        matches!(body.nodes[*ret as usize], PNode::Prim(Prim::Number)),
        "{body:?}"
    );
}

#[test]
fn export_braces_exports_a_declaration_written_without_export() {
    let out = run("interface Point { x: number } export { Point };");
    assert!(!out.incomplete, "{out:?}");
    assert_eq!(
        table(&out),
        vec![("Point", key("Point", DeclKind::Interface))]
    );
}

#[test]
fn export_braces_may_come_before_the_declaration() {
    let out = run("export { Point }; interface Point { x: number }");
    assert!(!out.incomplete, "{out:?}");
    assert_eq!(names(&out), vec!["Point"]);
}

#[test]
fn export_braces_may_rename() {
    let out = run("interface Point { x: number } export { Point as Vec2 };");
    assert!(!out.incomplete, "{out:?}");
    assert_eq!(&*out.exports[0].name, "Point");
    assert_eq!(
        table(&out),
        vec![("Vec2", key("Point", DeclKind::Interface))]
    );
}

#[test]
fn one_declaration_may_be_exported_under_two_names() {
    let out = run("interface P { x: number } export { P, P as Q };");
    assert!(!out.incomplete, "{out:?}");
    assert_eq!(out.exports.len(), 1, "one declaration, not two");
    assert_eq!(out.export_names.len(), 2);
}

#[test]
fn export_braces_of_a_name_that_is_a_type_and_a_value_exports_both() {
    let out = run("interface Foo { x: number } const Foo: number = 1; export { Foo };");
    assert!(!out.incomplete, "{out:?}");
    assert_eq!(out.exports.len(), 2);
    assert_eq!(out.export_names.len(), 2);
}

#[test]
fn export_braces_of_a_function_and_a_constant() {
    let out = run(
        "function add(a: number, b: number): number { return a + b; }
         const limit: number = 3;
         export { add, limit };",
    );
    assert!(!out.incomplete, "{out:?}");
    assert_eq!(names(&out), vec!["add", "limit"]);
}

#[test]
fn export_braces_of_an_unknown_name_is_unsupported() {
    assert!(run("export { Nothing };").incomplete);
}

#[test]
fn export_braces_of_an_unsupported_declaration_is_unsupported() {
    // A namespace has a value side the portable form does not carry yet.
    let out = run("namespace N { export const x = 1; } export { N };");
    assert!(out.incomplete);
    assert!(out.exports.is_empty());
}

#[test]
fn export_braces_of_an_unannotated_constant_exports_its_inferred_type() {
    let out = run("const n = 1; export { n };");
    assert!(!out.incomplete, "{out:?}");
    assert_eq!(names(&out), vec!["n"]);
}

#[test]
fn a_declaration_not_exported_by_either_route_stays_out() {
    let out = run("interface A { x: number } interface B { y: number } export { A };");
    assert_eq!(names(&out), vec!["A"]);
}

#[test]
fn an_enum_is_exported_as_a_declaration_of_kind_enum() {
    let out = run("export enum Color { Red, Green }");
    assert!(!out.incomplete, "{out:?}");
    assert_eq!(names(&out), vec!["Color"]);
    assert_eq!(out.exports[0].kind, DeclKind::Enum);
}

#[test]
fn an_enum_that_is_not_exported_is_nothing_to_us() {
    let out = run("enum Hidden { A } export interface P { x: number }");
    assert_eq!(names(&out), vec!["P"]);
    assert!(!out.incomplete);
}

#[test]
fn a_class_is_exported_as_an_instance_declaration_and_a_constructor() {
    let out = run("export class K { n: number = 1; }");
    assert!(!out.incomplete, "{out:?}");
    let mut kinds: Vec<_> = out.exports.iter().map(|d| d.kind).collect();
    kinds.sort_by_key(|k| *k as u8);
    assert_eq!(kinds, vec![DeclKind::Class, DeclKind::Var]);
    let public: Vec<&str> = out.export_names.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(public, vec!["K", "K"], "{out:?}");
}

#[test]
fn a_default_exported_class_is_listed_under_default() {
    let out = run("export default class Thing {}");
    assert!(!out.incomplete, "{out:?}");
    assert!(out.export_names.iter().all(|(n, _)| n == "default"));
    assert_eq!(out.export_names.len(), 2);
}

#[test]
fn a_default_exported_identifier_is_listed_under_default() {
    let out = run("const v = 1; export default v;");
    assert!(!out.incomplete, "{out:?}");
    let public: Vec<&str> = out.export_names.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(public, vec!["default"]);
}

// The key of a nested generic signature must not depend on where in the file it was
// written: an edit above a declaration moves every offset below it, and a hash that moved
// with them would tell a dependent that an unchanged signature had changed.
fn signature_of(source: &str, name: &str) -> tsox::SigHash {
    let out = run(source);
    out.exports
        .iter()
        .find(|decl| &*decl.name == name)
        .unwrap_or_else(|| panic!("{name} is not exported: {out:?}"))
        .sig
}

const BOX: &str = "export interface Box { map: <U>(f: (x: number) => U) => U[] }\n";

#[test]
fn lines_added_above_a_declaration_do_not_change_its_hash() {
    let plain = signature_of(BOX, "Box");
    let padded = signature_of(&format!("\n\n// a comment\n{BOX}"), "Box");
    assert_eq!(plain, padded);
}

#[test]
fn a_declaration_added_above_does_not_change_the_hash_of_the_one_below() {
    let alone = signature_of(BOX, "Box");
    let below = signature_of(
        &format!("export interface Other {{ id: number; name: string }}\n{BOX}"),
        "Box",
    );
    assert_eq!(alone, below);
}

#[test]
fn a_change_to_the_nested_signature_still_changes_the_hash() {
    let plain = signature_of(BOX, "Box");
    let bounded = signature_of(
        "export interface Box { map: <U extends string>(f: (x: number) => U) => U[] }\n",
        "Box",
    );
    assert_ne!(plain, bounded);
}

#[test]
fn two_nested_signatures_in_one_declaration_keep_distinct_keys() {
    let both = "export interface Pair { a: <T>(x: T) => T; b: <T>(x: T) => T[] }\n";
    let moved = format!("\n\n{both}");
    assert_eq!(signature_of(both, "Pair"), signature_of(&moved, "Pair"));
}

#[test]
fn default_expression_is_exported_under_default() {
    use tsox::PNode;
    let out = run("export default 42;");
    assert!(!out.incomplete, "{out:?}");
    assert_eq!(
        out.export_names
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["default"]
    );
    let body = body_of(&out, "default");
    assert!(
        matches!(&body.nodes[body.root as usize], PNode::NumLit(bits) if f64::from_bits(*bits) == 42.0)
    );
}

#[test]
fn destructured_exports_export_each_local_binding() {
    let out = run("export const { a, b: renamed } = { a: 1, b: 'x' };");
    assert!(!out.incomplete, "{out:?}");
    assert_eq!(names(&out), vec!["a", "renamed"]);
    assert_eq!(
        out.export_names
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["a", "renamed"]
    );
}

#[test]
fn external_declaration_files_do_not_implicitly_export_local_declarations() {
    let ctx = UnitContext::new(
        FileId::ROOT,
        "types.d.ts",
        "export {}; interface Hidden { value: number }",
    )
    .with_module(ModuleKey::project("types.d.ts"));
    let out = Worker::new().check_unit(&ctx).unwrap();
    assert!(!out.incomplete, "{out:?}");
    assert!(out.export_names.is_empty(), "{out:?}");
}
