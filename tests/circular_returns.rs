use tsox::{Diagnostic, TypeChecker};

// Problem: two functions with no return annotation whose return types are each read from
// the other have no answer, and the placeholder `any` they got was passed off as one with
// no error. tsc reports TS7023 on each function that is part of the circle.
// Picked: a call by name to a function declared in the same file, inside a `return`
// expression of an unannotated function, is noted, and a cycle among the notes is
// reported once on each function on it. Nothing else is read as a dependency: a call
// outside a `return`, a call inside a nested function, an annotated function.
// Cost: a circle through an import, a method or a variable is not seen and still reads as
// `any` (see tests/project_imports.rs for the cross-file case).
// Now: recursion inside a larger return expression, and mutual recursion, are reported. A
// bare `return f(...)` inside `f` is not, because tsc leaves it out of the return type, and
// neither is recursion with an annotation or a call that cannot change a return type.

fn diagnostics(source: &str) -> Vec<Diagnostic> {
    TypeChecker::new()
        .check_source(source, "case.ts")
        .expect("the source parses")
        .diagnostics
}

fn circular(source: &str) -> Vec<String> {
    let mut names: Vec<String> = diagnostics(source)
        .into_iter()
        .filter(|d| d.code.tsc_code() == Some(7023))
        .map(|d| d.message)
        .collect();
    names.sort();
    names
}

#[test]
fn two_functions_that_return_each_other_are_both_reported() {
    let found = circular("function f() { return g(); }\nfunction g() { return f(); }\n");
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(found[0].starts_with("'f' implicitly has return type 'any'"));
    assert!(found[1].starts_with("'g' implicitly has return type 'any'"));
}

#[test]
fn a_function_that_returns_a_call_to_itself_is_reported() {
    let found = circular("function fact(n: number) { return n <= 1 ? 1 : n * fact(n - 1); }\n");
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].starts_with("'fact' implicitly has return type 'any'"));
}

#[test]
fn a_circle_through_a_third_function_reports_all_three_once() {
    let found = circular(
        "function a() { return b(); }\nfunction b() { return c(); }\nfunction c() { return a(); }\n",
    );
    assert_eq!(found.len(), 3, "{found:?}");
}

#[test]
fn a_function_outside_the_circle_is_not_reported() {
    let found = circular(
        "function outside() { return f(); }\nfunction f() { return g(); }\nfunction g() { return f(); }\n",
    );
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(found.iter().all(|m| !m.starts_with("'outside'")));
}

#[test]
fn recursion_with_a_return_annotation_is_not_reported() {
    let found = circular(
        "function fact(n: number): number { return n <= 1 ? 1 : n * fact(n - 1); }\n\
         function f(): number { return g(); }\nfunction g(): number { return f(); }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_chain_with_no_circle_is_not_reported() {
    let found = circular("function f() { return g(); }\nfunction g() { return 1; }\n");
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_call_outside_a_return_expression_is_not_reported() {
    let found = circular(
        "function f(n: number) { if (n > 0) { f(n - 1); } return 1; }\n\
         function g() { const x = h(); return 1; }\nfunction h() { return g(); }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_call_inside_a_nested_function_is_not_reported() {
    let found = circular("function f() { return () => f(); }\n");
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_bare_tail_call_to_itself_is_not_reported() {
    let found = circular(
        "function gcd(a: number, b: number) { if (b === 0) return a; return gcd(b, a % b); }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_tail_call_is_not_reported_in_parentheses_or_on_a_later_branch() {
    let found = circular(
        "function r(n: number) { return (r(n - 1)); }\n\
         function s(n: number) { if (n) return s(n - 1); return 1; }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_self_call_inside_a_larger_expression_is_still_reported() {
    let found = circular(
        "function u(n: number) { return u(n - 1) + 1; }\n\
         function t(n: number) { return n ? t(n - 1) : 1; }\n",
    );
    assert_eq!(found.len(), 2, "{found:?}");
}

#[test]
fn a_self_call_as_an_argument_of_another_call_is_still_reported() {
    let found = circular(
        "function id(x: number) { return x; }\n\
         function z(n: number) { return id(z(n)); }\n",
    );
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].starts_with("'z' implicitly has return type 'any'"));
}

#[test]
fn a_tail_call_does_not_hide_a_circle_through_another_function() {
    let found = circular(
        "function f(n: number) { return g(n); }\nfunction g(n: number) { return f(n); }\n",
    );
    assert_eq!(found.len(), 2, "{found:?}");
}
