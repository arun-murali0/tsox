use tsox::{Severity, TypeChecker};

// Syntax errors the parser finds, reported in tsc's code and wording at the line tsc gives them.
// Each case was compared with tsc 6.0.3 (one file per case, so one error cannot change how the
// next is recovered). Only rules with one tsc code and one tsc text are translated; the rest of
// the parser's messages are still left out rather than reported under a guessed code.

fn errors(source: &str) -> Vec<(usize, String)> {
    let result = TypeChecker::new()
        .check_source(source, "case.ts")
        .expect("a file with syntax errors still reports them");
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

fn only(source: &str, line: usize, message: &str) {
    let found = errors(source);
    assert!(
        found.iter().any(|(l, m)| *l == line && m == message),
        "{message:?} at line {line} not in {found:?}"
    );
}

#[test]
fn a_unary_operand_of_an_exponent_is_ts17006() {
    only(
        "var x = -2 ** 2;\nexport {};\n",
        1,
        "An unary expression with the '-' operator is not allowed in the left-hand side of an exponentiation expression. Consider enclosing the expression in parentheses.",
    );
}

#[test]
fn modifier_rules_use_tscs_wording() {
    only(
        "class C { static public m() {} }\nexport {};\n",
        1,
        "'public' modifier must precede 'static' modifier.",
    );
    only(
        "class C { public private x = 1; }\nexport {};\n",
        1,
        "Accessibility modifier already seen.",
    );
    only(
        "class D { constructor(private [a]: number[]) {} }\nexport {};\n",
        1,
        "A parameter property may not be declared using a binding pattern.",
    );
}

#[test]
fn parameter_and_arrow_rules() {
    only(
        "function f(a?: number = 1) {}\nexport {};\n",
        1,
        "Parameter cannot have question mark and initializer.",
    );
    only(
        "var h = () \n=> 1;\nexport {};\n",
        2,
        "Line terminator not permitted before arrow.",
    );
}

#[test]
fn an_expected_token_is_ts1005_and_the_file_is_still_checked() {
    only("var o = { a: 1 b: 2 };\nexport {};\n", 1, "',' expected.");
}

#[test]
fn an_empty_type_argument_list_is_ts1099() {
    only(
        "var t: Array<> ;\nexport {};\n",
        1,
        "Type argument list cannot be empty.",
    );
}

// Rules the parser gives up on (it builds no tree), so the translated error is the only thing the
// file reports. Each source was compared with tsc 6.0.3, one file per case; tsc reports the
// same code and text on the same line (tsc also reports implicit-any errors for some of these
// files, which need the tree this parse does not have).

#[test]
fn a_rest_parameter_that_is_not_last_is_ts1014() {
    only(
        "function f(...a, b) {}\nexport {};\n",
        1,
        "A rest parameter must be last in a parameter list.",
    );
}

#[test]
fn a_rest_element_that_is_not_last_is_ts2462() {
    only(
        "let [...a, b] = [1, 2];\nexport {};\n",
        1,
        "A rest element must be last in a destructuring pattern.",
    );
}

#[test]
fn enum_member_name_rules_are_ts2452_and_ts1164() {
    only(
        "enum E { 1 = 0 }\nexport {};\n",
        1,
        "An enum member cannot have a numeric name.",
    );
    only(
        "enum E { [k] = 0 }\nexport {};\n",
        1,
        "Computed property names are not allowed in enums.",
    );
}

#[test]
fn index_signature_rules_are_ts1021_and_ts1019() {
    only(
        "interface I { [k: string]; }\nexport {};\n",
        1,
        "An index signature must have a type annotation.",
    );
    only(
        "interface I { [k?: string]: number; }\nexport {};\n",
        1,
        "An index signature parameter cannot have a question mark.",
    );
}

#[test]
fn a_repaired_rule_reports_the_syntax_error_and_what_follows_from_it() {
    // The misplaced rest parameter is repaired, so the file is checked: tsc reports TS1014 and
    // the implicit-any error for `b`, and nothing else.
    let found = errors("function f(...a, b) {}\nexport {};\n");
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(
        found
            .iter()
            .any(|(l, m)| *l == 1 && m.starts_with("A rest parameter"))
    );
}

#[test]
fn an_index_signature_with_a_bad_key_is_not_reported_as_ts1021() {
    // tsc reports an invalid key type as TS1268 and an optional key as TS1019, not TS1021, so
    // those stay unreported here rather than carry the wrong code.
    for source in [
        "var foo: { [index: any]; };\nexport {};\n",
        "interface I { [a: boolean]; }\nexport {};\n",
        "interface I { [k?: string]; }\nexport {};\n",
    ] {
        // Nothing translates, so the file is refused as before (an error, not diagnostics).
        match TypeChecker::new().check_source(source, "case.ts") {
            Err(_) => {}
            Ok(result) => assert!(
                result
                    .diagnostics
                    .iter()
                    .all(|d| d.message != "An index signature must have a type annotation."),
                "{source:?} reported TS1021"
            ),
        }
    }
    only(
        "interface I { [k: number]; }\nexport {};\n",
        1,
        "An index signature must have a type annotation.",
    );
}
