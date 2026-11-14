use tsox::{Severity, TypeChecker};

// Files oxc refuses are repaired and re-parsed, and tsc's syntax code is reported at the
// original position. Compared with tsc 6.0.3.

fn error_starts(source: &str) -> Vec<u32> {
    let result = TypeChecker::new()
        .check_source(source, "case.ts")
        .expect("should be checked");
    let mut starts: Vec<u32> = result
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.start)
        .collect();
    starts.sort();
    starts
}

#[test]
fn index_signatures_without_a_type_report_each_line() {
    let source = "interface A {\n  [x: string];\n  [y: number];\n}\n";
    let got = error_starts(source);
    let second = source.find("[y").unwrap() as u32;
    assert_eq!(got.len(), 2, "{got:?}");
    assert!(got[0] < second && got[1] >= second, "{got:?}");
}

#[test]
fn element_access_without_argument_is_reported_in_place() {
    let source = "var a: any;\nvar b = a[];\n";
    let got = error_starts(source);
    assert!(
        got.iter().any(|s| *s >= source.find("b =").unwrap() as u32),
        "{got:?}"
    );
}

// Compared with tsc 6.0.3: each keeps checking the rest of the file, as tsc does.

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

#[test]
fn a_numeric_enum_member_name_is_ts2452_and_the_file_is_still_checked() {
    let found = error_lines("enum E { 1 = 0 }\nlet x: number = \"a\";\n");
    assert!(
        found
            .iter()
            .any(|(l, m)| *l == 1 && m == "An enum member cannot have a numeric name."),
        "{found:?}"
    );
    assert!(
        found.iter().any(|(l, _)| *l == 2),
        "line 2 is checked: {found:?}"
    );
}

#[test]
fn a_computed_enum_member_name_is_ts1164_and_the_file_is_still_checked() {
    let found = error_lines("enum E { [k] = 0, B = 1 }\nlet x: E = E.B;\nlet y: number = \"a\";\n");
    assert!(
        found
            .iter()
            .any(|(l, m)| *l == 1 && m == "Computed property names are not allowed in enums."),
        "{found:?}"
    );
    assert!(
        found.iter().any(|(l, _)| *l == 3),
        "line 3 is checked: {found:?}"
    );
    assert!(
        !found.iter().any(|(l, _)| *l == 2),
        "E.B still resolves: {found:?}"
    );
}

#[test]
fn an_index_signature_with_a_bad_key_is_not_reported_as_ts1021() {
    // tsc reports an invalid key type as TS1268 and an optional key as TS1019.
    for source in [
        "var foo: { [index: any]; };\nexport {};\n",
        "interface I { [a: boolean]; }\nexport {};\n",
    ] {
        if let Ok(result) = TypeChecker::new().check_source(source, "case.ts") {
            assert!(
                result
                    .diagnostics
                    .iter()
                    .all(|d| d.message != "An index signature must have a type annotation."),
                "{source:?} reported TS1021"
            );
        }
    }
}

// Compared with tsc 6.0.3, one file per case: the same code on the same line, and the rest of
// the file is still checked.

const TS1014: &str = "A rest parameter must be last in a parameter list.";
const TS2364: &str =
    "The left-hand side of an assignment expression must be a variable or a property access.";
const TS2322: &str = "Type 'string' is not assignable to type 'number'.";

fn has(found: &[(u32, String)], line: u32, message: &str) -> bool {
    found.iter().any(|(l, m)| *l == line && m == message)
}

#[test]
fn a_rest_parameter_that_is_not_last_is_ts1014_and_the_file_is_still_checked() {
    let found = error_lines("function f(...a, b) {}\nlet x: number = \"s\";\n");
    assert!(has(&found, 1, TS1014), "{found:?}");
    assert!(has(&found, 2, TS2322), "{found:?}");
}

#[test]
fn an_annotated_misplaced_rest_parameter_keeps_its_array_type() {
    let found = error_lines(
        "function g(...r: string[], z: number) { return r.length; }\nlet x: number = \"s\";\n",
    );
    assert!(has(&found, 1, TS1014), "{found:?}");
    assert!(has(&found, 2, TS2322), "{found:?}");
    assert_eq!(
        found.iter().filter(|(l, _)| *l == 1).count(),
        1,
        "the repair must not invent errors in the body: {found:?}"
    );
}

#[test]
fn an_invalid_assignment_target_is_ts2364_on_each_line_and_the_file_is_still_checked() {
    let found = error_lines("1 = 2;\n1 >>= 2;\nlet x: number = \"s\";\n");
    assert!(has(&found, 1, TS2364), "{found:?}");
    assert!(has(&found, 2, TS2364), "{found:?}");
    assert!(has(&found, 3, TS2322), "{found:?}");
}

#[test]
fn a_parenthesized_invalid_target_is_reported_with_its_parentheses() {
    let source = "let y: any;\n(1, y) = 0;\nlet x: number = \"s\";\n";
    let result = TypeChecker::new()
        .check_source(source, "case.ts")
        .expect("should be checked");
    let at = source.find("(1, y)").unwrap() as u32;
    let found = error_lines(source);
    assert!(has(&found, 2, TS2364), "{found:?}");
    assert!(has(&found, 3, TS2322), "{found:?}");
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.message == TS2364 && d.start == at && d.end == at + 6),
        "TS2364 covers `(1, y)`"
    );
}

#[test]
fn an_increment_of_a_non_reference_is_left_alone() {
    // tsc reports TS2357 or TS2356 here, depending on the operand's type; no rule for that yet, so
    // the file is refused as before and never carries TS2364.
    for source in ["var a = --1;\n", "1 ++;\n"] {
        if let Ok(result) = TypeChecker::new().check_source(source, "case.ts") {
            assert!(
                result.diagnostics.iter().all(|d| d.message != TS2364),
                "{source:?} reported TS2364"
            );
        }
    }
}

// More repairs, each compared with tsc 6.0.3 for code, text and line, with the rest of the file
// still checked (the `let z: number = "s"` line).

const TAIL: &str = "let z: number = \"s\";\n";

fn with_tail(source: &str) -> String {
    format!("{source}{TAIL}")
}

#[test]
fn an_optional_object_member_is_ts1162_and_the_file_is_still_checked() {
    let found = error_lines(&with_tail("var v = { foo?() { } };\nvar w = { x ?: 1 };\n"));
    let optional = "An object member cannot be declared optional.";
    assert!(has(&found, 1, optional), "{found:?}");
    assert!(has(&found, 2, optional), "{found:?}");
    assert!(has(&found, 3, TS2322), "{found:?}");
}

#[test]
fn a_definite_assertion_on_an_object_member_is_ts1255() {
    let found = error_lines(&with_tail("let a = 1;\nconst foo = { a! };\n"));
    assert!(
        has(
            &found,
            2,
            "A definite assignment assertion '!' is not permitted in this context."
        ),
        "{found:?}"
    );
    assert!(has(&found, 3, TS2322), "{found:?}");
}

#[test]
fn an_enum_member_followed_by_a_colon_is_ts1357_and_its_value_is_the_next_member() {
    let found = error_lines(&with_tail("enum E3 { a: 1, b }\n"));
    assert!(
        has(
            &found,
            1,
            "An enum member name must be followed by a ',', '=', or '}'."
        ),
        "{found:?}"
    );
    assert!(
        has(&found, 1, "An enum member cannot have a numeric name."),
        "{found:?}"
    );
    assert!(has(&found, 2, TS2322), "{found:?}");
}

#[test]
fn a_catch_clause_initializer_is_ts1197_and_is_dropped() {
    let found = error_lines(&with_tail("try {} catch (e = 1) { }\n"));
    assert!(
        has(
            &found,
            1,
            "Catch clause variable cannot have an initializer."
        ),
        "{found:?}"
    );
    assert!(has(&found, 2, TS2322), "{found:?}");
}

#[test]
fn an_index_signature_parameter_initializer_is_ts1020() {
    let found = error_lines(&with_tail("interface I { [a: number = 1]: number; }\n"));
    assert!(
        has(
            &found,
            1,
            "An index signature parameter cannot have an initializer."
        ),
        "{found:?}"
    );
    assert!(has(&found, 2, TS2322), "{found:?}");
}

#[test]
fn a_compound_assignment_to_a_non_number_is_left_for_the_arithmetic_rule() {
    // tsc reports `this *= 1` as TS2362 (the left side of an arithmetic operation), never TS2364,
    // so this repair only takes `=` and number literals.
    for source in ["this *= 1;\n", "true **= 1;\n"] {
        if let Ok(result) = TypeChecker::new().check_source(source, "case.ts") {
            assert!(
                result.diagnostics.iter().all(|d| d.message != TS2364),
                "{source:?} reported TS2364"
            );
        }
    }
    let found = error_lines("1 >>= 2;\n");
    assert!(has(&found, 1, TS2364), "{found:?}");
}

#[test]
fn grammar_errors_are_silenced_in_a_file_with_a_parse_error() {
    // tsc reports its grammar errors (TS1014 here) only for a file its parser found nothing in. This
    // file has a missing `)` as well, which is a parse error, so TS1014 is not reported.
    // The file is refused outright (nothing translates), which is also "not reported".
    if let Ok(result) =
        TypeChecker::new().check_source("function f(...a, b) {}\nvar x = (1 + ;\n", "case.ts")
    {
        assert!(
            result.diagnostics.iter().all(|d| d.message != TS1014),
            "TS1014 should be silenced"
        );
    }
}

#[test]
fn a_type_keyword_used_as_a_value_is_ts2693() {
    let found = error_lines("var results = number[];\n");
    assert!(
        has(
            &found,
            1,
            "'number' only refers to a type, but is being used as a value here."
        ),
        "{found:?}"
    );
}

#[test]
fn repairs_of_different_kinds_in_one_file_each_report_at_their_own_line() {
    // Compared with tsc 6.0.3: the parser's TS1011 on line 2 hides the checker's grammar error
    // (TS1014, line 3), while the checker's TS2364 and TS2322 are still reported. Each repair
    // changes the length of the text after it.
    let source = "var a: any;\nvar b = a[];\nfunction f(...r: string[], z: number) {}\n1 = 2;\nlet x: number = \"s\";\n";
    let found = error_lines(source);
    assert!(
        has(
            &found,
            2,
            "An element access expression should take an argument."
        ),
        "{found:?}"
    );
    assert!(!found.iter().any(|(l, _)| *l == 3), "{found:?}");
    assert!(has(&found, 4, TS2364), "{found:?}");
    assert!(has(&found, 5, TS2322), "{found:?}");
}

#[test]
fn grammar_repairs_alone_are_all_reported() {
    // No parser error, so the grammar errors on lines 1 and 2 both stand (compared with tsc).
    let found = error_lines("function f(...r: string[], z: number) {}\nenum E { 1 = 0 }\n");
    assert!(has(&found, 1, TS1014), "{found:?}");
    assert!(
        has(&found, 2, "An enum member cannot have a numeric name."),
        "{found:?}"
    );
}

// tsc lists a file's diagnostics by position. The `[]` of `number[]` comes after `number`, so
// TS2693 (on `number`) comes before TS1011 (on the `]`), though the parser finds TS1011 first.
#[test]
fn diagnostics_are_listed_by_position_as_tsc_lists_them() {
    let result = TypeChecker::new()
        .check_source("var results = number[];\n", "case.ts")
        .expect("should be checked");
    let starts: Vec<u32> = result
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.start)
        .collect();
    let mut sorted = starts.clone();
    sorted.sort();
    assert_eq!(starts, sorted, "{starts:?}");
    assert!(starts.len() >= 2, "{starts:?}");
}

// Compared with tsc 6.0.3: `foo` inside the repaired left side is reported at its own column, and
// TS2364 covers the parenthesized left side from its first `(`.
#[test]
fn a_name_inside_a_repaired_assignment_target_keeps_its_position() {
    let source = "(foo()) = bar;\n";
    let result = TypeChecker::new()
        .check_source(source, "case.ts")
        .expect("should be checked");
    let at = |needle: &str| source.find(needle).unwrap() as u32;
    let foo = result
        .diagnostics
        .iter()
        .find(|d| d.message == "Cannot find name 'foo'.")
        .expect("foo is reported");
    assert_eq!((foo.start, foo.end), (at("foo"), at("foo") + 3));
    let target = result
        .diagnostics
        .iter()
        .find(|d| d.message == TS2364)
        .expect("TS2364 is reported");
    assert_eq!(target.start, 0);
    let order: Vec<&str> = result
        .diagnostics
        .iter()
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(order[0], TS2364, "{order:?}");
}

// Update expressions, `for` heads and rest elements, compared with tsc 6.0.3.

const TS2357: &str =
    "The operand of an increment or decrement operator must be a variable or a property access.";
const TS2356: &str =
    "An arithmetic operand must be of type 'any', 'number', 'bigint' or an enum type.";

#[test]
fn incrementing_a_number_literal_is_ts2357_and_the_file_is_still_checked() {
    let found = error_lines(&with_tail("1 ++;\nvar a = --1;\nvar b = ++(2);\n"));
    assert!(has(&found, 1, TS2357), "{found:?}");
    assert!(has(&found, 2, TS2357), "{found:?}");
    assert!(has(&found, 3, TS2357), "{found:?}");
    assert!(has(&found, 4, TS2322), "{found:?}");
}

#[test]
fn incrementing_a_boolean_or_string_is_ts2356_and_never_ts2357() {
    let found = error_lines(&with_tail("var a = --true;\nvar b = ++\"\";\n"));
    assert!(has(&found, 1, TS2356), "{found:?}");
    assert!(has(&found, 2, TS2356), "{found:?}");
    assert!(
        !found.iter().any(|(_, m)| m == TS2357),
        "tsc reports only TS2356 for these: {found:?}"
    );
}

#[test]
fn an_increment_of_an_operand_with_an_unknown_type_is_left_alone() {
    // `--f()` is TS2356 or TS2357 depending on what `f` returns, which the text does not say.
    if let Ok(result) =
        TypeChecker::new().check_source("declare function f(): any;\nvar a = --f();\n", "case.ts")
    {
        assert!(
            result
                .diagnostics
                .iter()
                .all(|d| d.message != TS2357 && d.message != TS2356),
            "no rule for an operand of unknown type"
        );
    }
}

#[test]
fn a_call_as_a_for_head_is_ts2406_or_ts2487() {
    let found = error_lines(&with_tail(
        "declare function foo(): any;\ndeclare var b: any;\nfor (foo() in b) { }\nfor (foo() of b) { }\n",
    ));
    assert!(
        has(
            &found,
            3,
            "The left-hand side of a 'for...in' statement must be a variable or a property access."
        ),
        "{found:?}"
    );
    assert!(
        has(
            &found,
            4,
            "The left-hand side of a 'for...of' statement must be a variable or a property access."
        ),
        "{found:?}"
    );
    assert!(has(&found, 5, TS2322), "{found:?}");
}

#[test]
fn a_rest_element_that_is_not_last_is_ts2462_and_the_file_is_still_checked() {
    let found = error_lines(&with_tail(
        "let [...a, b] = [1, 2];\nlet {...c, d} = {d: 1, e: 2};\n",
    ));
    let message = "A rest element must be last in a destructuring pattern.";
    assert!(has(&found, 1, message), "{found:?}");
    assert!(has(&found, 2, message), "{found:?}");
    assert!(has(&found, 3, TS2322), "{found:?}");
}
