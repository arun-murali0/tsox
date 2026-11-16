use tsox::{Severity, TypeChecker};

// Three small rules compared with tsc 6.0.3: a name that is really a member of the class around
// it (TS2662, TS2663), `arguments` across a class field initializer (TS2815), and `eval` or
// `arguments` bound or assigned where the code is strict (TS1215, TS1210).

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

fn at(found: &[(usize, String)], line: usize, starts_with: &str) -> bool {
    found
        .iter()
        .any(|(l, m)| *l == line && m.starts_with(starts_with))
}

#[test]
fn a_bare_member_name_gets_the_member_hint() {
    let source = "class C {\n  static foo = 1; bar = 2;\n  static sm() { return foo; }\n  im() { return bar; }\n  im2() { return foo; }\n  im3() { return () => bar; }\n  im4() { return function () { return bar; }; }\n  static sm2() { return bar; }\n  x = baz;\n}\nexport {};\n";
    let found = errors(source);
    assert!(
        at(
            &found,
            3,
            "Cannot find name 'foo'. Did you mean the static member 'C.foo'?"
        ),
        "{found:?}"
    );
    assert!(
        at(
            &found,
            4,
            "Cannot find name 'bar'. Did you mean the instance member 'this.bar'?"
        ),
        "{found:?}"
    );
    assert!(
        at(
            &found,
            5,
            "Cannot find name 'foo'. Did you mean the static member 'C.foo'?"
        ),
        "{found:?}"
    );
    assert!(
        at(
            &found,
            6,
            "Cannot find name 'bar'. Did you mean the instance member"
        ),
        "{found:?}"
    );
    // A nested function has its own `this`, and a static member has no instance to mean.
    assert!(
        at(&found, 7, "Cannot find name 'bar'.") && !at(&found, 7, "Cannot find name 'bar'. Did"),
        "{found:?}"
    );
    assert!(
        at(&found, 8, "Cannot find name 'bar'.") && !at(&found, 8, "Cannot find name 'bar'. Did"),
        "{found:?}"
    );
    assert!(at(&found, 9, "Cannot find name 'baz'."), "{found:?}");
}

#[test]
fn arguments_across_a_field_initializer() {
    let source = "class D { y = arguments; }\nfunction A() { return class T { a = arguments; b = () => arguments; c = function () { return arguments; }; }; }\nexport {};\n";
    let found = errors(source);
    assert!(
        at(
            &found,
            1,
            "Cannot find name 'arguments'. Did you mean the static member 'D.arguments'?"
        ),
        "{found:?}"
    );
    let in_function: Vec<_> = found
        .iter()
        .filter(|(l, m)| {
            *l == 2 && m.starts_with("'arguments' cannot be referenced in property initializers")
        })
        .collect();
    assert_eq!(in_function.len(), 2, "{found:?}");
}

#[test]
fn eval_and_arguments_cannot_be_bound_or_assigned_in_strict_code() {
    let source = "var eval = 1;\nfunction f(arguments: number) {}\ntry { } catch (arguments) { }\neval = 2;\nclass K { m() { var eval = 1; } }\nexport {};\n";
    let found = errors(source);
    for line in [1, 4] {
        assert!(
            at(
                &found,
                line,
                "Invalid use of 'eval'. Modules are automatically in strict mode."
            ),
            "{found:?}"
        );
    }
    for line in [2, 3] {
        assert!(
            at(
                &found,
                line,
                "Invalid use of 'arguments'. Modules are automatically in strict mode."
            ),
            "{found:?}"
        );
    }
    assert!(
        at(
            &found,
            5,
            "Code contained in a class is evaluated in JavaScript's strict mode"
        ),
        "{found:?}"
    );
}

#[test]
fn ambient_declarations_are_not_checked_for_eval_and_arguments() {
    // Compared with tsc 6.0.3: no error for a parameter named `arguments` in a `declare class`
    // or `declare function` (the binder skips ambient declarations), but one in a plain class.
    let source = "declare class A { constructor(arguments: number); }\ndeclare function f(eval: number): void;\nclass B { constructor(arguments: number) {} }\n";
    let result = TypeChecker::new()
        .check_source(source, "case.ts")
        .expect("checked");
    let lines: Vec<usize> = result
        .diagnostics
        .iter()
        .filter(|d| d.message.contains("strict mode"))
        .map(|d| source[..d.start as usize].matches('\n').count() + 1)
        .collect();
    assert_eq!(lines, vec![3], "{lines:?}");
}
