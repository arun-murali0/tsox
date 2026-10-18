use tsox::{Diagnostic, DiagnosticCode, Severity, TypeChecker};

// Dead code, and valid code that must not be mistaken for it, in every construct that
// holds a statement list: function, method, constructor, accessor and object-method
// bodies, arrow and function expressions (including callbacks), try/catch/finally, every
// kind of loop, labeled statements, switch cases and blocks, plus literal `true` / `false`
// conditions. Each dead fixture reports exactly one UnreachableCode at the line tsc
// reports; each clean fixture reports no error at all. Warnings are not counted: a
// `try` statement still carries a "not yet checked" warning that is not this suite's
// concern.

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!("fixtures/unreachable-scopes/", $name))
    };
}

fn errors(source: &str, file_name: &str) -> Vec<Diagnostic> {
    let result = TypeChecker::new().check_source(source, file_name);
    assert!(result.is_ok(), "fixture should at least parse: {result:?}");
    result
        .map(|checked| checked.diagnostics)
        .unwrap_or_default()
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .collect()
}

fn line_of(source: &str, offset: u32) -> usize {
    source[..offset as usize].matches('\n').count() + 1
}

fn assert_dead_at(file_name: &str, source: &str, expected_line: usize) {
    let diagnostics = errors(source, file_name);
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one error, got: {diagnostics:?}"
    );
    assert_eq!(diagnostics[0].code, DiagnosticCode::UnreachableCode);
    assert_eq!(
        line_of(source, diagnostics[0].start),
        expected_line,
        "reported on the wrong line: {diagnostics:?}"
    );
}

fn assert_reachable(file_name: &str, source: &str) {
    let diagnostics = errors(source, file_name);
    assert!(
        diagnostics.is_empty(),
        "expected no errors (this code is reachable), got: {diagnostics:?}"
    );
}

#[test]
fn dead_after_break() {
    assert_dead_at("after_break.ts", fixture!("after_break.ts"), 5);
}

#[test]
fn dead_after_continue() {
    assert_dead_at("after_continue.ts", fixture!("after_continue.ts"), 5);
}

#[test]
fn dead_arrow_block_dead_code() {
    assert_dead_at(
        "arrow_block_dead_code.ts",
        fixture!("arrow_block_dead_code.ts"),
        3,
    );
}

#[test]
fn dead_callback_arrow_dead_code() {
    assert_dead_at(
        "callback_arrow_dead_code.ts",
        fixture!("callback_arrow_dead_code.ts"),
        6,
    );
}

#[test]
fn dead_catch_block_dead_code() {
    assert_dead_at(
        "catch_block_dead_code.ts",
        fixture!("catch_block_dead_code.ts"),
        6,
    );
}

#[test]
fn dead_class_after_return() {
    assert_dead_at(
        "class_after_return.ts",
        fixture!("class_after_return.ts"),
        3,
    );
}

#[test]
fn dead_class_constructor_dead_code() {
    assert_dead_at(
        "class_constructor_dead_code.ts",
        fixture!("class_constructor_dead_code.ts"),
        4,
    );
}

#[test]
fn dead_class_getter_dead_code() {
    assert_dead_at(
        "class_getter_dead_code.ts",
        fixture!("class_getter_dead_code.ts"),
        4,
    );
}

#[test]
fn dead_class_method_dead_code() {
    assert_dead_at(
        "class_method_dead_code.ts",
        fixture!("class_method_dead_code.ts"),
        4,
    );
}

#[test]
fn dead_code_after_infinite_loop_is_unreachable() {
    assert_dead_at(
        "code_after_infinite_loop_is_unreachable.ts",
        fixture!("code_after_infinite_loop_is_unreachable.ts"),
        5,
    );
}

#[test]
fn dead_code_after_try_catch_all_return() {
    assert_dead_at(
        "code_after_try_catch_all_return.ts",
        fixture!("code_after_try_catch_all_return.ts"),
        7,
    );
}

#[test]
fn dead_const_after_return() {
    assert_dead_at(
        "const_after_return.ts",
        fixture!("const_after_return.ts"),
        3,
    );
}

#[test]
fn dead_do_while_body_dead_code() {
    assert_dead_at(
        "do_while_body_dead_code.ts",
        fixture!("do_while_body_dead_code.ts"),
        4,
    );
}

#[test]
fn dead_enum_and_declare_after() {
    assert_dead_at(
        "enum_and_declare_after.ts",
        fixture!("enum_and_declare_after.ts"),
        3,
    );
}

#[test]
fn dead_expression_after_throw() {
    assert_dead_at(
        "expression_after_throw.ts",
        fixture!("expression_after_throw.ts"),
        3,
    );
}

#[test]
fn dead_finally_block_dead_code() {
    assert_dead_at(
        "finally_block_dead_code.ts",
        fixture!("finally_block_dead_code.ts"),
        7,
    );
}

#[test]
fn dead_for_in_body_dead_code() {
    assert_dead_at(
        "for_in_body_dead_code.ts",
        fixture!("for_in_body_dead_code.ts"),
        4,
    );
}

#[test]
fn dead_for_of_body_dead_code() {
    assert_dead_at(
        "for_of_body_dead_code.ts",
        fixture!("for_of_body_dead_code.ts"),
        4,
    );
}

#[test]
fn dead_function_expression_dead_code() {
    assert_dead_at(
        "function_expression_dead_code.ts",
        fixture!("function_expression_dead_code.ts"),
        3,
    );
}

#[test]
fn dead_if_false_else_keeps_alternate() {
    assert_dead_at(
        "if_false_else_keeps_alternate.ts",
        fixture!("if_false_else_keeps_alternate.ts"),
        4,
    );
}

#[test]
fn dead_if_true_literal() {
    assert_dead_at("if_true_literal.ts", fixture!("if_true_literal.ts"), 5);
}

#[test]
fn dead_labeled_body_dead_code() {
    assert_dead_at(
        "labeled_body_dead_code.ts",
        fixture!("labeled_body_dead_code.ts"),
        4,
    );
}

#[test]
fn dead_nested_block_dead_code() {
    assert_dead_at(
        "nested_block_dead_code.ts",
        fixture!("nested_block_dead_code.ts"),
        4,
    );
}

#[test]
fn dead_object_method_dead_code() {
    assert_dead_at(
        "object_method_dead_code.ts",
        fixture!("object_method_dead_code.ts"),
        4,
    );
}

#[test]
fn dead_static_method_dead_code() {
    assert_dead_at(
        "static_method_dead_code.ts",
        fixture!("static_method_dead_code.ts"),
        4,
    );
}

#[test]
fn dead_switch_case_dead_code() {
    assert_dead_at(
        "switch_case_dead_code.ts",
        fixture!("switch_case_dead_code.ts"),
        5,
    );
}

#[test]
fn dead_try_block_dead_code() {
    assert_dead_at(
        "try_block_dead_code.ts",
        fixture!("try_block_dead_code.ts"),
        4,
    );
}

#[test]
fn dead_var_decl_after_return() {
    assert_dead_at(
        "var_decl_after_return.ts",
        fixture!("var_decl_after_return.ts"),
        3,
    );
}

#[test]
fn dead_while_false() {
    assert_dead_at("while_false.ts", fixture!("while_false.ts"), 4);
}

#[test]
fn reachable_after_switch_all_return_no_default() {
    assert_reachable(
        "after_switch_all_return_no_default.ts",
        fixture!("after_switch_all_return_no_default.ts"),
    );
}

#[test]
fn reachable_arrow_body_early_return() {
    assert_reachable(
        "arrow_body_early_return.ts",
        fixture!("arrow_body_early_return.ts"),
    );
}

#[test]
fn reachable_break_in_switch_in_while_true() {
    assert_reachable(
        "break_in_switch_in_while_true.ts",
        fixture!("break_in_switch_in_while_true.ts"),
    );
}

#[test]
fn reachable_callback_with_early_return() {
    assert_reachable(
        "callback_with_early_return.ts",
        fixture!("callback_with_early_return.ts"),
    );
}

#[test]
fn reachable_class_method_after_return_in_branch() {
    assert_reachable(
        "class_method_after_return_in_branch.ts",
        fixture!("class_method_after_return_in_branch.ts"),
    );
}

#[test]
fn reachable_closure_inside_loop_with_break() {
    assert_reachable(
        "closure_inside_loop_with_break.ts",
        fixture!("closure_inside_loop_with_break.ts"),
    );
}

#[test]
fn reachable_code_after_switch_with_break() {
    assert_reachable(
        "code_after_switch_with_break.ts",
        fixture!("code_after_switch_with_break.ts"),
    );
}

#[test]
fn reachable_code_after_switch_with_default_return_and_break() {
    assert_reachable(
        "code_after_switch_with_default_return_and_break.ts",
        fixture!("code_after_switch_with_default_return_and_break.ts"),
    );
}

#[test]
fn reachable_constructor_with_guard() {
    assert_reachable(
        "constructor_with_guard.ts",
        fixture!("constructor_with_guard.ts"),
    );
}

#[test]
fn reachable_do_while_then_code() {
    assert_reachable("do_while_then_code.ts", fixture!("do_while_then_code.ts"));
}

#[test]
fn reachable_do_while_true_break() {
    assert_reachable("do_while_true_break.ts", fixture!("do_while_true_break.ts"));
}

#[test]
fn reachable_empty_statement_after_return() {
    assert_reachable(
        "empty_statement_after_return.ts",
        fixture!("empty_statement_after_return.ts"),
    );
}

#[test]
fn reachable_for_ever_with_break_then_code() {
    assert_reachable(
        "for_ever_with_break_then_code.ts",
        fixture!("for_ever_with_break_then_code.ts"),
    );
}

#[test]
fn reachable_for_in_break_then_code() {
    assert_reachable(
        "for_in_break_then_code.ts",
        fixture!("for_in_break_then_code.ts"),
    );
}

#[test]
fn reachable_for_in_with_continue() {
    assert_reachable(
        "for_in_with_continue.ts",
        fixture!("for_in_with_continue.ts"),
    );
}

#[test]
fn reachable_for_of_continue_then_code() {
    assert_reachable(
        "for_of_continue_then_code.ts",
        fixture!("for_of_continue_then_code.ts"),
    );
}

#[test]
fn reachable_function_declarations_after_return_are_fine() {
    assert_reachable(
        "function_declarations_after_return_are_fine.ts",
        fixture!("function_declarations_after_return_are_fine.ts"),
    );
}

#[test]
fn reachable_if_else_return_then_nothing() {
    assert_reachable(
        "if_else_return_then_nothing.ts",
        fixture!("if_else_return_then_nothing.ts"),
    );
}

#[test]
fn reachable_if_true_without_exit_keeps_going() {
    assert_reachable(
        "if_true_without_exit_keeps_going.ts",
        fixture!("if_true_without_exit_keeps_going.ts"),
    );
}

#[test]
fn reachable_interface_and_type_after_return() {
    assert_reachable(
        "interface_and_type_after_return.ts",
        fixture!("interface_and_type_after_return.ts"),
    );
}

#[test]
fn reachable_labeled_break() {
    assert_reachable("labeled_break.ts", fixture!("labeled_break.ts"));
}

#[test]
fn reachable_labeled_continue_reachable() {
    assert_reachable(
        "labeled_continue_reachable.ts",
        fixture!("labeled_continue_reachable.ts"),
    );
}

#[test]
fn reachable_method_with_loops_and_returns() {
    assert_reachable(
        "method_with_loops_and_returns.ts",
        fixture!("method_with_loops_and_returns.ts"),
    );
}

#[test]
fn reachable_nested_arrow_functions() {
    assert_reachable(
        "nested_arrow_functions.ts",
        fixture!("nested_arrow_functions.ts"),
    );
}

#[test]
fn reachable_nested_function_after_return() {
    assert_reachable(
        "nested_function_after_return.ts",
        fixture!("nested_function_after_return.ts"),
    );
}

#[test]
fn reachable_object_method_early_return() {
    assert_reachable(
        "object_method_early_return.ts",
        fixture!("object_method_early_return.ts"),
    );
}

#[test]
fn reachable_return_in_catch_code_after_try() {
    assert_reachable(
        "return_in_catch_code_after_try.ts",
        fixture!("return_in_catch_code_after_try.ts"),
    );
}

#[test]
fn reachable_static_block_and_getter() {
    assert_reachable(
        "static_block_and_getter.ts",
        fixture!("static_block_and_getter.ts"),
    );
}

#[test]
fn reachable_switch_fallthrough_groups() {
    assert_reachable(
        "switch_fallthrough_groups.ts",
        fixture!("switch_fallthrough_groups.ts"),
    );
}

#[test]
fn reachable_switch_in_arrow() {
    assert_reachable("switch_in_arrow.ts", fixture!("switch_in_arrow.ts"));
}

#[test]
fn reachable_switch_inside_loop_continue() {
    assert_reachable(
        "switch_inside_loop_continue.ts",
        fixture!("switch_inside_loop_continue.ts"),
    );
}

#[test]
fn reachable_ternary_and_logical() {
    assert_reachable("ternary_and_logical.ts", fixture!("ternary_and_logical.ts"));
}

#[test]
fn reachable_throw_in_if_then_code() {
    assert_reachable(
        "throw_in_if_then_code.ts",
        fixture!("throw_in_if_then_code.ts"),
    );
}

#[test]
fn reachable_try_catch_finally_all_reachable() {
    assert_reachable(
        "try_catch_finally_all_reachable.ts",
        fixture!("try_catch_finally_all_reachable.ts"),
    );
}

#[test]
fn reachable_try_catch_then_code() {
    assert_reachable("try_catch_then_code.ts", fixture!("try_catch_then_code.ts"));
}

#[test]
fn reachable_try_finally_then_code() {
    assert_reachable(
        "try_finally_then_code.ts",
        fixture!("try_finally_then_code.ts"),
    );
}

#[test]
fn reachable_try_loop_break_in_catch() {
    assert_reachable(
        "try_loop_break_in_catch.ts",
        fixture!("try_loop_break_in_catch.ts"),
    );
}

#[test]
fn reachable_try_return_catch_return_finally() {
    assert_reachable(
        "try_return_catch_return_finally.ts",
        fixture!("try_return_catch_return_finally.ts"),
    );
}

#[test]
fn reachable_var_without_initializer_after_return() {
    assert_reachable(
        "var_without_initializer_after_return.ts",
        fixture!("var_without_initializer_after_return.ts"),
    );
}

#[test]
fn reachable_while_cond_continue() {
    assert_reachable("while_cond_continue.ts", fixture!("while_cond_continue.ts"));
}

#[test]
fn reachable_while_true_break_in_nested_switch() {
    assert_reachable(
        "while_true_break_in_nested_switch.ts",
        fixture!("while_true_break_in_nested_switch.ts"),
    );
}

#[test]
fn reachable_while_true_return_inside_then_nothing() {
    assert_reachable(
        "while_true_return_inside_then_nothing.ts",
        fixture!("while_true_return_inside_then_nothing.ts"),
    );
}

#[test]
fn reachable_while_true_with_break_then_code() {
    assert_reachable(
        "while_true_with_break_then_code.ts",
        fixture!("while_true_with_break_then_code.ts"),
    );
}
