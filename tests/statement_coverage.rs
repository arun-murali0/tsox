use tsox::{Diagnostic, Severity, TypeChecker};

// Statements that used to hit the "not yet checked" warning, or whose declarations were
// never registered: exports, try/catch/finally, throw, do-while, for-of, for-in,
// labeled statements, nested function declarations, and functions with no return
// annotation. Each fixture is either clean (no diagnostic of any severity, so a leftover
// "not yet checked" warning also fails it) or reports exactly one error that names
// the real problem inside the construct.

fn check(source: &str, file_name: &str) -> Vec<Diagnostic> {
    let result = TypeChecker::new().check_source(source, file_name);
    assert!(result.is_ok(), "fixture should at least parse: {result:?}");
    result
        .map(|checked| checked.diagnostics)
        .unwrap_or_default()
}

fn assert_clean(source: &str, file_name: &str) {
    let diagnostics = check(source, file_name);
    assert!(
        diagnostics.is_empty(),
        "expected no diagnostics, got: {diagnostics:?}"
    );
}

fn assert_one_error(source: &str, file_name: &str, expected_text: &str) {
    let diagnostics = check(source, file_name);
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert_eq!(diagnostics[0].severity, Severity::Error);
    assert!(
        diagnostics[0].message.contains(expected_text),
        "expected a message containing {expected_text:?}, got: {diagnostics:?}"
    );
}

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!("fixtures/statement-coverage/", $name, ".ts"))
    };
}

#[test]
fn exported_declarations_are_declared_and_clean() {
    assert_clean(
        fixture!("exported_declarations_are_declared_and_clean"),
        "exported_declarations_are_declared_and_clean.ts",
    );
}

#[test]
fn catch_parameter_and_throw_are_usable() {
    assert_clean(
        fixture!("catch_parameter_and_throw_are_usable"),
        "catch_parameter_and_throw_are_usable.ts",
    );
}

#[test]
fn for_of_binds_the_element_type() {
    assert_clean(
        fixture!("for_of_binds_the_element_type"),
        "for_of_binds_the_element_type.ts",
    );
}

#[test]
fn nested_function_is_hoisted_and_checked() {
    assert_clean(
        fixture!("nested_function_is_hoisted_and_checked"),
        "nested_function_is_hoisted_and_checked.ts",
    );
}

#[test]
fn unannotated_return_is_inferred() {
    assert_clean(
        fixture!("unannotated_return_is_inferred"),
        "unannotated_return_is_inferred.ts",
    );
}

#[test]
fn exported_function_return_is_checked() {
    assert_one_error(
        fixture!("exported_function_return_is_checked"),
        "exported_function_return_is_checked.ts",
        "not assignable",
    );
}

#[test]
fn exported_function_call_is_typed() {
    assert_one_error(
        fixture!("exported_function_call_is_typed"),
        "exported_function_call_is_typed.ts",
        "not assignable",
    );
}

#[test]
fn try_block_is_checked() {
    assert_one_error(
        fixture!("try_block_is_checked"),
        "try_block_is_checked.ts",
        "not assignable",
    );
}

#[test]
fn throw_argument_is_checked() {
    assert_one_error(
        fixture!("throw_argument_is_checked"),
        "throw_argument_is_checked.ts",
        "Cannot find name 'missing'",
    );
}

#[test]
fn for_of_element_type_is_checked() {
    assert_one_error(
        fixture!("for_of_element_type_is_checked"),
        "for_of_element_type_is_checked.ts",
        "not assignable",
    );
}

#[test]
fn for_in_key_is_a_string() {
    assert_one_error(
        fixture!("for_in_key_is_a_string"),
        "for_in_key_is_a_string.ts",
        "not assignable",
    );
}

#[test]
fn do_while_body_is_checked() {
    assert_one_error(
        fixture!("do_while_body_is_checked"),
        "do_while_body_is_checked.ts",
        "not assignable",
    );
}

#[test]
fn labeled_loop_body_is_checked() {
    assert_one_error(
        fixture!("labeled_loop_body_is_checked"),
        "labeled_loop_body_is_checked.ts",
        "not assignable",
    );
}

#[test]
fn nested_function_body_is_checked() {
    assert_one_error(
        fixture!("nested_function_body_is_checked"),
        "nested_function_body_is_checked.ts",
        "not assignable",
    );
}

#[test]
fn unannotated_return_mismatch_is_reported() {
    assert_one_error(
        fixture!("unannotated_return_mismatch_is_reported"),
        "unannotated_return_mismatch_is_reported.ts",
        "not assignable",
    );
}

#[test]
fn unannotated_without_return_is_void() {
    assert_one_error(
        fixture!("unannotated_without_return_is_void"),
        "unannotated_without_return_is_void.ts",
        "not assignable",
    );
}
