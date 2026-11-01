use tsox::{Diagnostic, TypeChecker};

// Cases from the first 500 files of the TypeScript repository's tests/cases (compiler, then
// conformance, sorted by path), each reduced to what tsc accepts. Every one of these was a
// false error before. A source that tsc rejects is not here; those are gaps, not false errors.

fn check(source: &str) -> Vec<Diagnostic> {
    let result = TypeChecker::new().check_source(source, "case.ts");
    assert!(result.is_ok(), "should parse: {result:?}");
    result.map(|c| c.diagnostics).unwrap_or_default()
}

fn assert_clean(source: &str) {
    let found = check(source);
    assert!(found.is_empty(), "expected no diagnostics, got: {found:?}");
}

// A function and a class constructor have the members of `Function`.
#[test]
fn prototype_of_a_class_can_be_read_and_assigned_through() {
    assert_clean(
        "declare class Point { add(dx: number, dy: number): void; }\n\
         Point.prototype.add = function(dx: number, dy: number) {};\n",
    );
}

#[test]
fn a_constructor_is_assignable_to_an_object_type_with_prototype() {
    assert_clean(
        "declare class XEvent { x: number; }\n\
         var v: { prototype: XEvent } = XEvent;\n",
    );
}

#[test]
fn a_function_has_name_and_length() {
    assert_clean(
        "function f(a: number) { return a; }\n\
         export const n: string = f.name;\n\
         export const l: number = f.length;\n",
    );
}

// `toLocaleString` takes the optional locales and options of `Intl`.
#[test]
fn array_to_locale_string_takes_locales_and_options() {
    assert_clean(
        "const arr = [1, 2, 3];\n\
         let s: string;\n\
         s = arr.toLocaleString();\n\
         s = arr.toLocaleString('en-US');\n\
         s = arr.toLocaleString('en-US', { style: 'currency', currency: 'EUR' });\n",
    );
}

// A target with a rest parameter can be called with any number of arguments, so a source that
// needs one is still assignable to it.
#[test]
fn a_function_with_a_required_parameter_fits_a_rest_only_target() {
    assert_clean(
        "declare const fa: (s: string, ...args: string[]) => string;\n\
         const f1: (...args: any) => string = fa;\n\
         const f2: (...args: any[]) => string = fa;\n",
    );
}

#[test]
fn a_required_parameter_fits_an_optional_one_in_the_target() {
    assert_clean(
        "declare const g: (a: number) => void;\n\
         const h: (a?: number) => void = g;\n",
    );
}

// `any` absorbs the other members of a union, so a method of `any || []` is any.
#[test]
fn an_any_or_empty_array_keeps_calling_like_any() {
    assert_clean(
        "declare const x: any;\n\
         const y = x || [];\n\
         y.map((s: string) => s);\n",
    );
}

// Overload signatures form the callable type; the implementation signature is
// only for checking the body and is not callable.
#[test]
fn overloads_resolve_to_the_first_matching_signature() {
    let diags = check(
        "function f(a: string, y): number;\n\
         function f(a: string, x): string;\n\
         function f(a: any): any { return a; }\n\
         export const n: number = f(\"s\", null);\n",
    );
    assert!(
        !diags.iter().any(|d| d.message.contains("Expected")),
        "{diags:?}"
    );
}

#[test]
fn an_overload_result_is_checked_against_its_matching_signature() {
    let diags = check(
        "function f(a: string): number;\n\
         function f(a: number): string;\n\
         function f(a: any): any { return a; }\n\
         export const z: number = f(1);\n",
    );
    assert!(
        diags.iter().any(|d| d.message.contains("not assignable")),
        "{diags:?}"
    );
}

// `interface Array<T> { ... }` in a program adds to the built-in type.
#[test]
fn a_program_can_augment_the_array_interface() {
    assert_clean(
        "interface Array<T> { split: (parts: number) => T[][]; }\n\
         var x = [''];\n\
         export const y: string[][] = x.split(4);\n",
    );
}

#[test]
fn a_program_can_augment_object_and_function() {
    assert_clean(
        "interface Object { data: number; }\n\
         interface Function { functionData: string; }\n\
         var f = function () {};\n\
         export const a: string = f.functionData;\n\
         export const b: number = f.data;\n",
    );
}

#[test]
fn an_empty_interface_accepts_arrays_and_primitives() {
    assert_clean(
        "interface IState {}\n\
         declare function f(s: IState): void;\n\
         declare const a: string[];\n\
         f(a);\n\
         const n: IState = 1;\n",
    );
}

#[test]
fn a_generic_rest_parameter_infers_the_argument_parameters() {
    assert_clean(
        "declare function goofus<ARGS extends any[]>(f: (...args: ARGS) => any): void;\n\
         goofus((a: string) => 1);\n",
    );
}

#[test]
fn an_async_arrow_returns_a_promise() {
    assert_clean(
        "const h = async () => 1;\n\
         export const p: Promise<number> = h();\n",
    );
}

#[test]
fn a_value_of_a_constrained_type_parameter_is_called_through_its_constraint() {
    assert_clean(
        "export function f<T extends (...args: any[]) => number>(fn: T): number { return fn(1, 2); }\n",
    );
}

// A file checked alone has nothing to import from, so a specifier is missing unless the
// file declares the module itself.
#[test]
fn a_standalone_import_of_a_missing_module_is_reported() {
    let diags = check("import { x } from \"./nowhere\";\nexport { x };\n");
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("Cannot find module")),
        "{diags:?}"
    );
}

#[test]
fn a_standalone_import_of_a_declared_module_is_not() {
    let diags = check(
        "declare module \"m\" { export const x: number; }\n\
         import { x } from \"m\";\nexport const y: number = x;\n",
    );
    assert!(
        !diags
            .iter()
            .any(|d| d.message.contains("Cannot find module")),
        "{diags:?}"
    );
}

// TS7008, TS7010, TS7013 and the signatures' own parameters need no contextual type.
#[test]
fn members_and_bodyless_signatures_without_types_are_implicit_any() {
    let diags = check(
        "class A { a; }\n\
         interface I { m(); new (y); }\n\
         declare function f(z);\n",
    );
    let codes: Vec<u32> = diags.iter().filter_map(|d| d.code.tsc_code()).collect();
    for wanted in [7008, 7010, 7013, 7006] {
        assert!(codes.contains(&wanted), "{wanted} missing in {diags:?}");
    }
}

// tsc's binder: each declaration brings a meaning, and some meanings cannot share a name.
fn duplicate_lines(source: &str) -> Vec<(u32, String)> {
    let mut lines: Vec<(u32, String)> = check(source)
        .iter()
        .filter(|d| matches!(d.code.tsc_code(), Some(2300) | Some(2451)))
        .map(|d| {
            let line = source[..d.start as usize].matches('\n').count() as u32 + 1;
            (line, d.message.clone())
        })
        .collect();
    lines.sort();
    lines
}

#[test]
fn a_class_and_a_variable_of_one_name_are_both_reported() {
    let lines = duplicate_lines("class c1 {}\nvar c1 = 1;\n");
    assert_eq!(lines.len(), 2, "{lines:?}");
}

#[test]
fn a_function_and_a_namespace_merge_and_so_do_two_vars() {
    let lines = duplicate_lines(
        "function f() {}\nnamespace f { export var x = 1; }\nvar v; var v;\ninterface I {} class I {}\n",
    );
    assert!(lines.is_empty(), "{lines:?}");
}

#[test]
fn a_let_redeclared_by_a_var_is_a_block_scoped_error() {
    let lines = duplicate_lines("let q; var q;\n");
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(
        lines.iter().all(|(_, m)| m.contains("block-scoped")),
        "{lines:?}"
    );
}

#[test]
fn an_inner_block_may_shadow() {
    let lines = duplicate_lines("{ let b; { let b; } }\nfunction f(a) { { let a; } }\n");
    assert!(lines.is_empty(), "{lines:?}");
}

#[test]
fn a_repeated_enum_member_is_reported_in_both_declarations() {
    let lines = duplicate_lines("enum E { One }\nenum E { One }\n");
    assert_eq!(lines.len(), 2, "{lines:?}");
}

#[test]
fn a_bodyless_accessor_is_not_an_implicit_any_return() {
    let diags = check("abstract class A { abstract get p(); }\n");
    assert!(
        !diags.iter().any(|d| d.code.tsc_code() == Some(7010)),
        "{diags:?}"
    );
}

// A class whose members carry no annotations is still a class: its fields are the type of
// a literal initializer or `any`, and its methods return `any`.
#[test]
fn an_unannotated_class_keeps_its_shape() {
    let diags = check(
        "class A { n = 1; s = \"x\"; m() { return 1; } }\n\
         const a = new A();\n\
         export const bad: string = a.n;\n",
    );
    assert!(
        diags.iter().any(|d| d.message.contains("not assignable")),
        "{diags:?}"
    );
    assert!(
        !diags.iter().any(|d| d.message.contains("not yet checked")),
        "{diags:?}"
    );
}

// A user-defined type guard narrows in both branches; an assertion function narrows what
// follows it, including by a property of the argument.
#[test]
fn a_type_predicate_narrows_both_branches() {
    assert_clean(
        "interface Cat { type: 'cat'; meow: true; }\n\
         interface Dog { type: 'dog'; bark: true; }\n\
         function isCat(a: Cat | Dog): a is Cat { return a.type === 'cat'; }\n\
         export function f(a: Cat | Dog) { if (isCat(a)) { a.meow; } else { a.bark; } }\n",
    );
}

#[test]
fn an_assertion_function_narrows_what_follows() {
    assert_clean(
        "declare function assertString(x: unknown): asserts x is string;\n\
         export function g(v: string | number) { assertString(v); const s: string = v; }\n",
    );
}

#[test]
fn an_assertion_on_a_discriminant_property_narrows_the_object() {
    assert_clean(
        "interface Cat { type: 'cat'; meow: true; }\n\
         interface Dog { type: 'dog'; bark: true; }\n\
         declare function assertEqual<T>(value: any, type: T): asserts value is T;\n\
         declare const animal: Cat | Dog | undefined;\n\
         assertEqual(animal?.type, 'cat' as const);\n\
         animal.meow;\n",
    );
}

// `const { kind, a } = x` keeps `kind` and `a` tied together across a union.
#[test]
fn destructured_discriminant_narrows_its_siblings() {
    assert_clean(
        "type X = { kind: \"a\", a: [1] } | { kind: \"b\", a: [] };\n\
         export function foo(x: X): 1 {\n\
           const { kind, a } = x;\n\
           switch (kind) { case \"a\": return a[0]; default: return 1; }\n\
         }\n",
    );
}
