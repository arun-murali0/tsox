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
