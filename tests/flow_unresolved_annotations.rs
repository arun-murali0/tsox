use tsox::{CheckOptions, DiagnosticCode, FileId, Severity, TypeChecker, UnitContext, Worker};

// Two flow checks that used to go quiet when the checker could not model a declared type.
// TS2454 only needs to know the type cannot be `undefined`, which a class, interface or enum
// declared in the program settles by itself; TS2564 needs the class's own type parameters in
// scope, and the constructor that has a body when there are overloads. Each case was compared
// with tsc 6.0.3. Some shapes draw a "not yet checked" warning, so these look at errors only.

fn errors(source: &str) -> Vec<(usize, String)> {
    let result = TypeChecker::new()
        .check_source(source, "case.ts")
        .expect("should parse");
    result
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| {
            let line = source[..d.start as usize].matches('\n').count() + 1;
            (line, d.message.clone())
        })
        .collect()
}

// TS2454 needs strictNullChecks, which the default options leave off.
fn unassigned_lines(source: &str) -> Vec<usize> {
    let options = CheckOptions {
        strict: true,
        strict_null_checks: true,
        ..CheckOptions::default()
    };
    let mut worker = Worker::new();
    let ctx = UnitContext::new(FileId::ROOT, "t.ts", source).with_options(options);
    let outcome = worker.check_unit(&ctx).expect("the unit checks");
    let mut found: Vec<usize> = outcome
        .diagnostics
        .iter()
        .filter(|d| d.code == DiagnosticCode::UsedBeforeAssigned)
        .map(|d| source[..d.start as usize].matches('\n').count() + 1)
        .collect();
    found.sort_unstable();
    found
}

fn lines_of(found: &[(usize, String)], code_text: &str) -> Vec<usize> {
    found
        .iter()
        .filter(|(_, message)| message.contains(code_text))
        .map(|(line, _)| *line)
        .collect()
}

#[test]
fn a_read_of_an_unassigned_class_typed_variable_is_reported_even_when_the_class_is_not_modelled() {
    // `foo(): this` and the quoted member name are shapes the checker does not model yet.
    let source = "class C { x = 1; foo(): this { return this; } }\ninterface I { a: number; \"a b\": number }\nvar c: C;\nvar i: I;\nvar r = i[\"a b\"];\nvar z = c.foo().x;\n";
    assert_eq!(unassigned_lines(source), vec![5, 6]);
}

#[test]
fn an_unassigned_enum_typed_variable_is_reported() {
    assert_eq!(
        unassigned_lines("enum E { A }\nfunction f() { let e: E; return e; }\n"),
        vec![2]
    );
}

#[test]
fn a_type_alias_or_type_parameter_still_waits_for_the_checker() {
    // An alias that includes `undefined` may be read before it is assigned.
    let found = unassigned_lines(
        "type MaybeNum = number | undefined;\nfunction f() { let m: MaybeNum; return m; }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_property_typed_by_a_class_type_parameter_is_reported() {
    let found = errors(
        "class A<T> { x: T; }\nclass D<T> { z: T | undefined; ok: T; constructor(v: T) { this.ok = v; } }\n",
    );
    assert_eq!(lines_of(&found, "has no initializer"), vec![1], "{found:?}");
}

#[test]
fn a_property_assigned_by_the_implementation_of_an_overloaded_constructor_is_not_reported() {
    let found = errors(
        "class B<T> {\n  a: T;\n  constructor(x: T, y?: T);\n  constructor(x: T);\n  constructor(x: T) { this.a = x; }\n}\n",
    );
    assert!(
        lines_of(&found, "has no initializer").is_empty(),
        "{found:?}"
    );
}

#[test]
fn a_read_of_an_unassigned_variable_typed_by_a_qualified_name_is_reported() {
    // Each declared type is a class, interface or enum exported from a namespace of this file,
    // reached through one to three names. `lav.Al` is an alias that may be `undefined`, and
    // `lav.Nope` is not there at all: neither is a TS2454. Compared with tsc 6.0.3.
    let source = "namespace lav { export class X { m(): this { return this; } } export interface J { a: number } export type Al = string | undefined; export namespace deep { export enum E { A } } }\nnamespace a.b { export class C { x = 1; } }\ndeclare namespace amb { class K {} }\nfunction f() {\n  var x: lav.X; x;\n  var j: lav.J; j;\n  var e: lav.deep.E; e;\n  var c: a.b.C; c;\n  var k: amb.K; k;\n  var al: lav.Al; al;\n  var nx: lav.Nope; nx;\n}\n";
    assert_eq!(unassigned_lines(source), vec![5, 6, 7, 8, 9]);
}
