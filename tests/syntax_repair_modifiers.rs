use tsox::{Severity, TypeChecker};

// A class member modifier where a statement belongs: TS1128 on the modifier, and what follows is
// read as a statement. Compared with tsc 6.0.3.

const TS2322: &str = "Type 'string' is not assignable to type 'number'.";

fn error_lines(source: &str) -> Vec<(u32, String)> {
    let result = TypeChecker::new()
        .check_source(source, "case.ts")
        .expect("should be checked");
    result
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| {
            (
                source[..d.start as usize].matches('\n').count() as u32 + 1,
                d.message.clone(),
            )
        })
        .collect()
}

fn has(found: &[(u32, String)], line: u32, message: &str) -> bool {
    found.iter().any(|(l, m)| *l == line && m == message)
}

#[test]
fn a_member_modifier_in_a_function_body_is_ts1128() {
    let found = error_lines(
        "module M {\n    function g() {\n        private y = 1;\n    }\n}\nfunction h() {\n  public p1 = 0;\n  static q = 2;\n}\nlet z: number = \"s\";\n",
    );
    let message = "Declaration or statement expected.";
    assert!(has(&found, 3, message), "{found:?}");
    assert!(has(&found, 7, message), "{found:?}");
    assert!(has(&found, 8, message), "{found:?}");
    assert!(has(&found, 10, TS2322), "{found:?}");
}

#[test]
fn a_modifier_word_that_is_a_name_is_not_a_modifier() {
    // `static` here is a variable's name, not a modifier before a statement.
    let found = error_lines("var static = 1;\nlet z: number = \"s\";\n");
    assert!(
        !found
            .iter()
            .any(|(_, m)| m == "Declaration or statement expected."),
        "{found:?}"
    );
}

// In a method or constructor tsc ends the body at the modifier: the modifier starts a class member
// and the body's own closing brace is left over, so there are two TS1128 and no unknown name.
// Another block inside a class is left alone.
#[test]
fn a_member_modifier_in_a_method_ends_the_body_and_leaves_a_stray_brace() {
    let message = "Declaration or statement expected.";
    for source in [
        "class C {\n  m() {\n    static x = 1;\n  }\n}\nlet z: number = \"s\";\n",
        "class C { constructor() { static f = 3; } }\nlet z: number = \"s\";\n",
    ] {
        let found = error_lines(source);
        assert_eq!(
            found.iter().filter(|(_, m)| m == message).count(),
            2,
            "{found:?}"
        );
        assert!(
            !found.iter().any(|(_, m)| m.starts_with("Cannot find name")),
            "{found:?}"
        );
        assert!(
            found.iter().any(|(_, m)| m == TS2322),
            "the rest of the file is still checked: {found:?}"
        );
    }
}

#[test]
fn a_rest_parameter_initializer_is_ts1048_and_dropped() {
    let found = error_lines("let f = (...arg: number[] = []) => 1;\nlet z: number = \"s\";\n");
    assert!(
        has(&found, 1, "A rest parameter cannot have an initializer."),
        "{found:?}"
    );
    assert!(has(&found, 2, TS2322), "{found:?}");
}

#[test]
fn a_using_declaration_is_not_an_invalid_assignment_target() {
    // `await using [a] = x` is a declaration oxc reads as an index expression; tsc's errors for it
    // are different, so no TS2364 is reported.
    if let Ok(result) = TypeChecker::new().check_source(
        "async function f() {\n  await using [a] = null;\n}\n",
        "case.ts",
    ) {
        assert!(
            result.diagnostics.iter().all(|d| !d
                .message
                .starts_with("The left-hand side of an assignment expression")),
            "reported TS2364"
        );
    }
}

#[test]
fn a_return_outside_a_function_does_not_check_its_expression() {
    // tsc leaves a top-level `return` statement unchecked, so the unknown name after it is not
    // reported. Here the stray `}` is a parse error, so tsc reports no TS1108 either: only the
    // TS1128 for the brace.
    let found = error_lines("return foo;\n}\nlet z: number = \"s\";\n");
    assert!(
        has(&found, 2, "Declaration or statement expected."),
        "{found:?}"
    );
    assert!(
        !found.iter().any(|(_, m)| m.starts_with("Cannot find name")),
        "{found:?}"
    );
    assert!(
        !found
            .iter()
            .any(|(_, m)| m.starts_with("A 'return' statement can only")),
        "{found:?}"
    );
}
