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
