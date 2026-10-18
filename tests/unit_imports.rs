use std::collections::HashMap;
use std::sync::Arc;

use tsox::{
    DeclKey, DeclProvider, FileId, ImportSource, ModuleKey, PortableDecl, Severity, UnitContext,
    UnitOutput, Worker,
};

// An import declaration binds the name to the dependency's real export. The project is
// built by hand here, from the outputs of real unit runs, which is what check_project
// does for a whole project (tests/project_imports.rs).

#[derive(Default)]
struct Project {
    // specifier as written in the importing file -> (export name, key)
    modules: HashMap<String, Vec<(String, DeclKey)>>,
    decls: HashMap<DeclKey, Arc<PortableDecl>>,
}

impl Project {
    fn add(&mut self, specifier: &str, out: &UnitOutput) {
        self.modules
            .insert(specifier.to_string(), out.export_names.clone());
        for decl in &out.exports {
            self.decls.insert(decl.key, Arc::new(decl.clone()));
        }
    }
}

impl DeclProvider for Project {
    fn decl(&self, key: DeclKey) -> Option<Arc<PortableDecl>> {
        self.decls.get(&key).cloned()
    }
}

impl ImportSource for Project {
    fn export_keys(&self, specifier: &str, name: &str) -> Vec<DeclKey> {
        self.modules
            .get(specifier)
            .into_iter()
            .flatten()
            .filter(|(n, _)| n == name)
            .map(|(_, k)| *k)
            .collect()
    }
}

fn unit(file: &str, source: &str, project: Option<&Project>) -> UnitOutput {
    let mut ctx =
        UnitContext::new(FileId::ROOT, file, source).with_module(ModuleKey::project(file));
    if let Some(p) = project {
        ctx = ctx.with_imports(p);
    }
    Worker::new().check_unit(&ctx).unwrap()
}

// `a.ts` checked first, then `b.ts` against it.
fn two(a: &str, b: &str) -> (UnitOutput, UnitOutput) {
    let a_out = unit("a.ts", a, None);
    assert!(a_out.diagnostics.is_empty(), "{:?}", a_out.diagnostics);
    let mut project = Project::default();
    project.add("./a", &a_out);
    let b_out = unit("b.ts", b, Some(&project));
    (a_out, b_out)
}

fn errors(out: &UnitOutput) -> Vec<String> {
    out.diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.message.clone())
        .collect()
}

fn clean(a: &str, b: &str) {
    let (_, out) = two(a, b);
    assert!(out.diagnostics.is_empty(), "{:?}", out.diagnostics);
}

fn fails(a: &str, b: &str) -> Vec<String> {
    let (_, out) = two(a, b);
    let errs = errors(&out);
    assert!(!errs.is_empty(), "expected an error, got none");
    errs
}

const USER: &str = "export interface User { name: string }";

#[test]
fn an_imported_interface_is_the_real_one() {
    // The first 6c fixture: a.ts exports an interface and b.ts misuses it.
    let errs = fails(
        USER,
        "import { User } from './a'; const u: User = { name: 1 };",
    );
    assert!(errs[0].contains("not assignable"), "{errs:?}");
}

#[test]
fn an_imported_interface_accepts_a_correct_use() {
    clean(
        USER,
        "import { User } from './a'; const u: User = { name: 'a' }; const n: string = u.name;",
    );
}

#[test]
fn a_missing_member_of_an_imported_interface_is_reported() {
    fails(
        USER,
        "import { User } from './a'; const u: User = { name: 'a' }; const n = u.age;",
    );
}

#[test]
fn an_import_can_be_renamed() {
    clean(
        USER,
        "import { User as Person } from './a'; const p: Person = { name: 'a' };",
    );
    fails(
        USER,
        "import { User as Person } from './a'; const p: Person = { name: 2 };",
    );
}

#[test]
fn an_imported_function_has_its_declared_signature() {
    let a = "export function add(a: number, b: number): number { return a + b; }";
    clean(a, "import { add } from './a'; const n: number = add(1, 2);");
    fails(a, "import { add } from './a'; const s: string = add(1, 2);");
    fails(a, "import { add } from './a'; add(1);");
}

#[test]
fn an_imported_constant_has_its_declared_type() {
    let a = "export const limit: number = 3;";
    clean(a, "import { limit } from './a'; const n: number = limit;");
    fails(a, "import { limit } from './a'; const s: string = limit;");
}

#[test]
fn an_imported_alias_is_usable() {
    let a = "export type Id = string; export type Pair = { a: number; b: string };";
    clean(
        a,
        "import { Id, Pair } from './a'; const i: Id = 'x'; const p: Pair = { a: 1, b: 'x' };",
    );
    fails(a, "import { Id } from './a'; const i: Id = 1;");
}

#[test]
fn import_type_brings_in_types() {
    clean(
        USER,
        "import type { User } from './a'; const u: User = { name: 'a' };",
    );
    clean(
        USER,
        "import { type User } from './a'; const u: User = { name: 'a' };",
    );
}

#[test]
fn a_recursive_imported_interface_works() {
    let a = "export interface Node { value: number; next: Node | null }";
    clean(
        a,
        "import { Node } from './a'; const n: Node = { value: 1, next: { value: 2, next: null } };",
    );
    fails(
        a,
        "import { Node } from './a'; const n: Node = { value: 1, next: { value: 'x', next: null } };",
    );
}

#[test]
fn an_imported_declaration_that_names_another_resolves_through_it() {
    let (_, out) = two(
        "export interface User { name: string }
         export function greet(u: User): string { return u.name; }",
        "import { User, greet } from './a';
         const u: User = { name: 'a' };
         const s: string = greet(u);
         const bad: number = greet(u);",
    );
    assert_eq!(errors(&out).len(), 1, "{:?}", out.diagnostics);
}

// -- generics: the AST-free instantiation ------------------------------------------

const BOX: &str = "export interface Box<T> { value: T }";

#[test]
fn an_imported_generic_can_be_instantiated_with_new_arguments() {
    clean(
        BOX,
        "import { Box } from './a';
         const n: Box<number> = { value: 1 };
         const s: Box<string> = { value: 'x' };",
    );
}

#[test]
fn each_instantiation_of_an_imported_generic_is_checked_against_its_own_argument() {
    fails(
        BOX,
        "import { Box } from './a'; const n: Box<number> = { value: 'x' };",
    );
    fails(
        BOX,
        "import { Box } from './a'; const s: Box<string> = { value: 1 };",
    );
}

#[test]
fn an_imported_generic_reads_back_the_argument_it_was_given() {
    clean(
        BOX,
        "import { Box } from './a'; const b: Box<number> = { value: 1 }; const n: number = b.value;",
    );
    fails(
        BOX,
        "import { Box } from './a'; const b: Box<number> = { value: 1 }; const s: string = b.value;",
    );
}

#[test]
fn an_imported_generic_alias_is_instantiated_too() {
    let a = "export type Pair<A, B> = { first: A; second: B };";
    clean(
        a,
        "import { Pair } from './a'; const p: Pair<number, string> = { first: 1, second: 'x' };",
    );
    fails(
        a,
        "import { Pair } from './a'; const p: Pair<number, string> = { first: 'x', second: 'x' };",
    );
}

#[test]
fn an_imported_default_fills_an_omitted_argument() {
    let a = "export interface Box<T = string> { value: T }";
    clean(
        a,
        "import { Box } from './a'; const b: Box = { value: 'x' };",
    );
    fails(a, "import { Box } from './a'; const b: Box = { value: 1 };");
    // An explicit argument still wins over the default.
    clean(
        a,
        "import { Box } from './a'; const b: Box<number> = { value: 1 };",
    );
}

#[test]
fn an_imported_default_may_name_an_earlier_parameter() {
    let a = "export type Pair<A, B = A> = { first: A; second: B };";
    clean(
        a,
        "import { Pair } from './a'; const p: Pair<number> = { first: 1, second: 2 };",
    );
    fails(
        a,
        "import { Pair } from './a'; const p: Pair<number> = { first: 1, second: 'x' };",
    );
}

#[test]
fn an_imported_bound_is_enforced_on_an_explicit_argument() {
    let a = "export interface Tagged<T extends string> { tag: T }";
    clean(
        a,
        "import { Tagged } from './a'; const t: Tagged<string> = { tag: 'x' };",
    );
    let errs = fails(
        a,
        "import { Tagged } from './a'; const t: Tagged<number> = { tag: 1 };",
    );
    assert!(errs[0].contains("does not satisfy"), "{errs:?}");
}

#[test]
fn the_wrong_number_of_arguments_to_an_imported_generic_is_reported() {
    let a = "export interface Pair<A, B> { first: A; second: B }";
    let errs = fails(
        a,
        "import { Pair } from './a'; const p: Pair<number> = { first: 1, second: 2 };",
    );
    assert!(errs.iter().any(|e| e.contains("Pair<A, B>")), "{errs:?}");
}

#[test]
fn a_bare_imported_generic_without_a_default_is_an_error() {
    fails(
        BOX,
        "import { Box } from './a'; const b: Box = { value: 1 };",
    );
}

#[test]
fn an_imported_generic_function_infers_at_the_call() {
    let a = "export function id<T>(x: T): T { return x; }";
    clean(
        a,
        "import { id } from './a'; const n: number = id(1); const s: string = id('x');",
    );
    fails(a, "import { id } from './a'; const n: number = id('x');");
}

// -- what an importing file exports --------------------------------------------------

#[test]
fn a_file_may_export_a_function_that_names_an_imported_type() {
    let (a, b) = two(
        USER,
        "import { User } from './a';
         export function name(u: User): string { return u.name; }",
    );
    assert!(b.diagnostics.is_empty(), "{:?}", b.diagnostics);
    assert!(!b.incomplete, "{b:?}");
    assert_eq!(b.exports.len(), 1);
    // The reference points at a.ts's declaration, by a.ts's key.
    let user_key = a.exports[0].key;
    assert!(
        b.exports[0].body.nodes.iter().any(|n| matches!(
            n,
            tsox::PNode::DeclRef { key, .. } if *key == user_key
        )),
        "{:?}",
        b.exports[0]
    );
}

// -- when nothing is found -------------------------------------------------------------

#[test]
fn an_unknown_specifier_binds_nothing_and_does_not_panic() {
    let a = unit("a.ts", USER, None);
    let mut project = Project::default();
    project.add("./a", &a);
    let out = unit(
        "b.ts",
        "import { User } from './nowhere'; const u: User = { name: 1 };",
        Some(&project),
    );
    assert!(
        errors(&out).iter().all(|e| !e.contains("not assignable")),
        "an unbound name must not produce a false mismatch: {:?}",
        out.diagnostics
    );
}

#[test]
fn a_name_the_module_does_not_export_binds_nothing() {
    let (_, out) = two(
        USER,
        "import { Missing } from './a'; const m: Missing = { x: 1 };",
    );
    assert!(
        errors(&out).iter().all(|e| !e.contains("not assignable")),
        "{:?}",
        out.diagnostics
    );
}

#[test]
fn without_an_import_source_an_import_declaration_is_still_unsupported() {
    let out = unit("b.ts", "import { User } from './a';", None);
    assert!(
        out.diagnostics
            .iter()
            .any(|d| d.message.contains("ImportDeclaration")),
        "{:?}",
        out.diagnostics
    );
}

#[test]
fn a_unit_checked_against_a_project_matches_one_checked_alone_when_it_imports_nothing() {
    let source = "const n: number = 'x';";
    let alone = unit("c.ts", source, None);
    let project = Project::default();
    let with = unit("c.ts", source, Some(&project));
    assert_eq!(alone.diagnostics.len(), with.diagnostics.len());
}

// -- default imports, and passing a name on --------------------------------------------

#[test]
fn a_default_import_binds_the_default_export() {
    clean(
        "export default interface User { name: string }",
        "import User from './a'; const u: User = { name: 'a' };",
    );
    fails(
        "export default interface User { name: string }",
        "import User from './a'; const u: User = { name: 1 };",
    );
}

#[test]
fn a_default_import_may_use_any_local_name() {
    clean(
        "export default interface User { name: string }",
        "import Person from './a'; const p: Person = { name: 'a' };",
    );
}

#[test]
fn a_default_imported_function_has_its_signature() {
    let a = "export default function area(w: number, h: number): number { return w * h; }";
    clean(a, "import area from './a'; const n: number = area(2, 3);");
    fails(a, "import area from './a'; const s: string = area(2, 3);");
}

#[test]
fn a_default_and_a_named_import_in_one_declaration() {
    let a = "export default interface User { name: string }
             export interface Role { id: number }";
    clean(
        a,
        "import User, { Role } from './a';
         const u: User = { name: 'a' };
         const r: Role = { id: 1 };",
    );
}

#[test]
fn a_name_exported_through_braces_is_importable() {
    clean(
        "interface Point { x: number } export { Point as Vec2 };",
        "import { Vec2 } from './a'; const v: Vec2 = { x: 1 };",
    );
    fails(
        "interface Point { x: number } export { Point as Vec2 };",
        "import { Vec2 } from './a'; const v: Vec2 = { x: 'a' };",
    );
}

// a.ts declares, b.ts passes it on, c.ts imports from b.ts.
fn chain(a: &str, b: &str, c: &str) -> (UnitOutput, UnitOutput) {
    let a_out = unit("a.ts", a, None);
    let mut project = Project::default();
    project.add("./a", &a_out);
    let b_out = unit("b.ts", b, Some(&project));
    assert!(b_out.diagnostics.is_empty(), "{:?}", b_out.diagnostics);
    assert!(!b_out.incomplete, "{b_out:?}");
    project.add("./b", &b_out);
    let c_out = unit("c.ts", c, Some(&project));
    (b_out, c_out)
}

#[test]
fn an_imported_name_exported_again_reaches_the_third_file() {
    let (b, c) = chain(
        USER,
        "import { User } from './a'; export { User };",
        "import { User } from './b'; const u: User = { name: 1 };",
    );
    assert!(b.exports.is_empty(), "b declares nothing of its own: {b:?}");
    assert!(!errors(&c).is_empty(), "{:?}", c.diagnostics);
}

#[test]
fn an_imported_name_exported_again_accepts_a_correct_use() {
    let (_, c) = chain(
        USER,
        "import { User } from './a'; export { User };",
        "import { User } from './b'; const u: User = { name: 'a' };",
    );
    assert!(c.diagnostics.is_empty(), "{:?}", c.diagnostics);
}

#[test]
fn a_re_export_from_another_module_reaches_the_third_file() {
    let (_, c) = chain(
        USER,
        "export { User } from './a';",
        "import { User } from './b'; const u: User = { name: 1 };",
    );
    assert!(!errors(&c).is_empty(), "{:?}", c.diagnostics);
}

#[test]
fn a_re_export_may_rename() {
    let (_, c) = chain(
        USER,
        "export { User as Person } from './a';",
        "import { Person } from './b'; const p: Person = { name: 'a' }; const bad: Person = { name: 2 };",
    );
    assert_eq!(errors(&c).len(), 1, "{:?}", c.diagnostics);
}

#[test]
fn a_re_export_from_an_unknown_module_is_unsupported() {
    let a = unit("a.ts", USER, None);
    let mut project = Project::default();
    project.add("./a", &a);
    let b = unit("b.ts", "export { User } from './nowhere';", Some(&project));
    assert!(b.incomplete);
    assert!(b.export_names.is_empty());
}

#[test]
fn a_generic_passed_on_keeps_its_parameters() {
    let (_, c) = chain(
        "export interface Box<T = string> { value: T }",
        "import { Box } from './a'; export { Box };",
        "import { Box } from './b';
         const s: Box = { value: 'x' };
         const n: Box<number> = { value: 1 };
         const bad: Box<number> = { value: 'x' };",
    );
    assert_eq!(errors(&c).len(), 1, "{:?}", c.diagnostics);
}

#[test]
fn an_imported_generic_function_preserves_signature_defaults() {
    clean(
        "export declare function make<T = string>(): T;",
        "import { make } from './a'; const text: string = make();",
    );
}
