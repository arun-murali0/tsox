use tsox::{DiagnosticCode, Severity, TypeChecker};

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
}

fn check(fixture_source: &str, file_name: &str) -> Vec<tsox::Diagnostic> {
    init_tracing();
    let checker = TypeChecker::new();
    let result = checker.check_source(fixture_source, file_name);
    assert!(result.is_ok(), "fixture should at least parse: {result:?}");
    result
        .map(|checked| checked.diagnostics)
        .unwrap_or_default()
}

#[test]
fn fully_annotated_arrow_function_checks_clean() {
    let source =
        include_str!("fixtures/expression-program-checking/arrow_function_fully_annotated.ts");
    let diagnostics = check(source, "arrow_function_fully_annotated.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn arrow_function_return_type_mismatch_is_caught() {
    let source =
        include_str!("fixtures/expression-program-checking/arrow_function_return_type_mismatch.ts");
    let diagnostics = check(source, "arrow_function_return_type_mismatch.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
}

#[test]
fn untyped_arrow_param_defaults_to_any_instead_of_blocking_the_function() {
    let source = include_str!(
        "fixtures/expression-program-checking/arrow_function_untyped_param_defaults_to_any.ts"
    );
    let diagnostics = check(source, "arrow_function_untyped_param_defaults_to_any.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "expected only the implicit-any error and nothing blocked downstream, got: {diagnostics:?}"
    );
    assert_eq!(diagnostics[0].severity, Severity::Error);
    assert!(
        diagnostics[0]
            .message
            .contains("Parameter 'x' implicitly has an 'any' type"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn expression_bodied_arrow_return_type_is_inferred_and_checked() {
    let source = include_str!(
        "fixtures/expression-program-checking/arrow_expression_body_return_inferred.ts"
    );
    let diagnostics = check(source, "arrow_expression_body_return_inferred.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic (the `bad` binding), got: {diagnostics:?}"
    );
}

#[test]
fn function_expression_works_as_a_value() {
    let source =
        include_str!("fixtures/expression-program-checking/function_expression_as_value.ts");
    let diagnostics = check(source, "function_expression_as_value.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn callback_parameter_type_annotation_end_to_end() {
    let source =
        include_str!("fixtures/expression-program-checking/callback_parameter_type_annotation.ts");
    let diagnostics = check(source, "callback_parameter_type_annotation.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn any_typed_value_is_callable() {
    let source =
        include_str!("fixtures/expression-program-checking/any_typed_value_is_callable.ts");
    let diagnostics = check(source, "any_typed_value_is_callable.ts");

    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert!(
        diagnostics[0].message.contains("arithmetic operation"),
        "expected the `1 - \"x\"` mismatch inside the call, got: {diagnostics:?}"
    );
    assert_eq!(diagnostics[0].severity, tsox::Severity::Error);
}

#[test]
fn function_expression_does_not_inherit_this() {
    let source = include_str!(
        "fixtures/expression-program-checking/function_expression_does_not_inherit_this.ts"
    );
    let diagnostics = check(source, "function_expression_does_not_inherit_this.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "`this` inside the nested function expression must not resolve to Counter's instance \
         type; it is an implicit any, and that is the one error. Got: {diagnostics:?}"
    );
    assert_eq!(diagnostics[0].severity, tsox::Severity::Error);
    assert!(
        diagnostics[0]
            .message
            .contains("'this' implicitly has type 'any'"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn this_in_an_object_literal_function_is_not_reported() {
    let source = include_str!(
        "fixtures/expression-program-checking/this_in_object_literal_function_is_not_implicit_any.ts"
    );
    let diagnostics = check(
        source,
        "this_in_object_literal_function_is_not_implicit_any.ts",
    );
    assert!(
        diagnostics.is_empty(),
        "an object literal method's `this` is the literal, got: {diagnostics:?}"
    );
}

#[test]
fn callback_type_annotation_with_untyped_param_still_registers() {
    let source = include_str!(
        "fixtures/expression-program-checking/callback_type_annotation_with_untyped_param.ts"
    );
    let diagnostics = check(source, "callback_type_annotation_with_untyped_param.ts");

    assert_eq!(
        diagnostics.len(),
        2,
        "expected the body mismatch plus the implicit-any error on the callback type, got: {diagnostics:?}"
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message.contains("not assignable")),
        "expected the `const result: string = fn(value)` mismatch inside apply's body, got: {diagnostics:?}"
    );
    let implicit_any = diagnostics
        .iter()
        .find(|d| d.message.contains("implicitly has an 'any' type"));
    assert_eq!(
        implicit_any.map(|diagnostic| diagnostic.severity),
        Some(Severity::Error),
        "expected an implicit-any error for the callback's `x`, got: {diagnostics:?}"
    );
}

#[test]
fn and_narrows_right_operand_using_left_truthy_slice() {
    let source =
        include_str!("fixtures/expression-program-checking/logical_and_narrows_right_operand.ts");
    let diagnostics = check(source, "logical_and_narrows_right_operand.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn and_result_type_includes_left_falsy_slice() {
    let source = include_str!(
        "fixtures/expression-program-checking/logical_and_result_type_includes_left_falsy_slice.ts"
    );
    let diagnostics = check(
        source,
        "logical_and_result_type_includes_left_falsy_slice.ts",
    );
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert!(
        diagnostics[0].message.contains("is not assignable to type"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn or_result_type_is_narrowed_union_and_checks_clean() {
    let source = include_str!(
        "fixtures/expression-program-checking/logical_or_result_type_is_narrowed_union.ts"
    );
    let diagnostics = check(source, "logical_or_result_type_is_narrowed_union.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn or_result_type_mismatch_is_caught() {
    let source =
        include_str!("fixtures/expression-program-checking/logical_or_result_type_mismatch.ts");
    let diagnostics = check(source, "logical_or_result_type_mismatch.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert!(
        diagnostics[0].message.contains("not assignable"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn nullish_coalescing_result_type_checks_clean() {
    let source = include_str!(
        "fixtures/expression-program-checking/logical_nullish_coalescing_result_type.ts"
    );
    let diagnostics = check(source, "logical_nullish_coalescing_result_type.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn nullish_coalescing_result_type_mismatch_is_caught() {
    let source = include_str!(
        "fixtures/expression-program-checking/logical_nullish_coalescing_result_type_mismatch.ts"
    );
    let diagnostics = check(source, "logical_nullish_coalescing_result_type_mismatch.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert!(
        diagnostics[0].message.contains("not assignable"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn ternary_narrows_each_branch_and_checks_clean() {
    let source =
        include_str!("fixtures/expression-program-checking/ternary_narrows_each_branch.ts");
    let diagnostics = check(source, "ternary_narrows_each_branch.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn ternary_result_type_mismatch_is_caught() {
    let source =
        include_str!("fixtures/expression-program-checking/ternary_result_type_mismatch.ts");
    let diagnostics = check(source, "ternary_result_type_mismatch.ts");
    // tsc reports a ternary mismatch on each offending branch (TS2322 twice here).
    assert_eq!(
        diagnostics.len(),
        2,
        "expected one diagnostic per branch, got: {diagnostics:?}"
    );
    assert!(
        diagnostics
            .iter()
            .all(|d| d.message.contains("is not assignable to type")),
        "got: {diagnostics:?}"
    );
    assert_ne!(
        diagnostics[0].start, diagnostics[1].start,
        "got: {diagnostics:?}"
    );
}

#[test]
fn narrowing_survives_early_return_from_if_with_no_else() {
    let source =
        include_str!("fixtures/expression-program-checking/narrowing_survives_early_return.ts");
    let diagnostics = check(source, "narrowing_survives_early_return.ts");
    assert!(
        diagnostics.is_empty(),
        "expected the gap to be closed, got: {diagnostics:?}"
    );
}

#[test]
fn as_expression_changes_the_tracked_type_and_checks_clean() {
    let source =
        include_str!("fixtures/expression-program-checking/as_expression_changes_tracked_type.ts");
    let diagnostics = check(source, "as_expression_changes_tracked_type.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn as_expression_does_not_block_downstream_errors() {
    let source = include_str!(
        "fixtures/expression-program-checking/as_expression_does_not_block_downstream_errors.ts"
    );
    let diagnostics = check(source, "as_expression_does_not_block_downstream_errors.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert!(
        diagnostics[0].message.contains("is not assignable to type"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn non_null_assertion_strips_nullish_and_checks_clean() {
    let source =
        include_str!("fixtures/expression-program-checking/non_null_assertion_strips_nullish.ts");
    let diagnostics = check(source, "non_null_assertion_strips_nullish.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn non_null_assertion_result_type_mismatch_is_caught() {
    let source = include_str!(
        "fixtures/expression-program-checking/non_null_assertion_result_type_mismatch.ts"
    );
    let diagnostics = check(source, "non_null_assertion_result_type_mismatch.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert!(
        diagnostics[0].message.contains("not assignable"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn computed_member_access_with_string_literal_key_checks_clean() {
    let source = include_str!(
        "fixtures/expression-program-checking/computed_member_access_string_literal_key.ts"
    );
    let diagnostics = check(source, "computed_member_access_string_literal_key.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn computed_member_access_with_dynamic_key_is_reported_as_unsupported() {
    let source = include_str!(
        "fixtures/expression-program-checking/computed_member_access_dynamic_key_unsupported.ts"
    );
    let diagnostics = check(source, "computed_member_access_dynamic_key_unsupported.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert_eq!(diagnostics[0].severity, tsox::Severity::Error);
    assert!(
        diagnostics[0]
            .message
            .contains("Element implicitly has an 'any' type"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn computed_member_access_with_an_any_key_is_implicit_any() {
    let source = include_str!(
        "fixtures/expression-program-checking/computed_member_access_any_key_is_implicit_any.ts"
    );
    let diagnostics = check(source, "computed_member_access_any_key_is_implicit_any.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert_eq!(diagnostics[0].severity, tsox::Severity::Error);
    assert!(
        diagnostics[0]
            .message
            .contains("Element implicitly has an 'any' type"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn computed_member_access_with_a_literal_typed_key_is_still_reported_as_unsupported() {
    let source = include_str!(
        "fixtures/expression-program-checking/computed_member_access_unsupported_key_stays_a_warning.ts"
    );
    let diagnostics = check(
        source,
        "computed_member_access_unsupported_key_stays_a_warning.ts",
    );
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert_eq!(diagnostics[0].severity, tsox::Severity::Warning);
    assert!(
        diagnostics[0].message.contains("not yet checked"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn optional_chaining_result_includes_undefined_and_checks_clean() {
    let source = include_str!(
        "fixtures/expression-program-checking/optional_chaining_result_includes_undefined.ts"
    );
    let diagnostics = check(source, "optional_chaining_result_includes_undefined.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn optional_chaining_result_type_mismatch_is_caught() {
    let source = include_str!(
        "fixtures/expression-program-checking/optional_chaining_result_type_mismatch.ts"
    );
    let diagnostics = check(source, "optional_chaining_result_type_mismatch.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert!(
        diagnostics[0].message.contains("is not assignable to type"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn optional_computed_member_access_checks_clean() {
    let source =
        include_str!("fixtures/expression-program-checking/optional_computed_member_access.ts");
    let diagnostics = check(source, "optional_computed_member_access.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn enum_numeric_auto_increment_checks_clean() {
    let source =
        include_str!("fixtures/expression-program-checking/enum_numeric_auto_increment.ts");
    let diagnostics = check(source, "enum_numeric_auto_increment.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn enum_type_position_is_member_union_and_checks_clean() {
    let source =
        include_str!("fixtures/expression-program-checking/enum_type_position_is_member_union.ts");
    let diagnostics = check(source, "enum_type_position_is_member_union.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn enum_string_members_check_clean() {
    let source = include_str!("fixtures/expression-program-checking/enum_string_members.ts");
    let diagnostics = check(source, "enum_string_members.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn enum_member_value_type_mismatch_is_caught() {
    let source =
        include_str!("fixtures/expression-program-checking/enum_member_value_type_mismatch.ts");
    let diagnostics = check(source, "enum_member_value_type_mismatch.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert!(
        diagnostics[0].message.contains("is not assignable to type"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn enum_constant_expression_initializer_is_evaluated() {
    let source = include_str!(
        "fixtures/expression-program-checking/enum_computed_initializer_unsupported.ts"
    );
    let diagnostics = check(source, "enum_computed_initializer_unsupported.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "`1 << 0` is a constant, so Weird resolves and returning it as a string is a mismatch, got: {diagnostics:?}"
    );
    assert_eq!(diagnostics[0].severity, tsox::Severity::Error);
    assert!(
        diagnostics[0].message.contains("is not assignable to type"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn enum_members_can_refer_to_earlier_members_and_auto_increment_after_them() {
    let source = include_str!(
        "fixtures/expression-program-checking/enum_constant_expression_initializers.ts"
    );
    let diagnostics = check(source, "enum_constant_expression_initializers.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "only `next` returns the enum as a string, got: {diagnostics:?}"
    );
    assert!(
        diagnostics[0].message.contains("is not assignable to type"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn enum_with_a_non_constant_initializer_is_honestly_left_unresolved() {
    let source = include_str!(
        "fixtures/expression-program-checking/enum_non_constant_initializer_unsupported.ts"
    );
    let diagnostics = check(source, "enum_non_constant_initializer_unsupported.ts");
    assert!(
        diagnostics.is_empty(),
        "expected the unsupported enum to be silently unresolved, got: {diagnostics:?}"
    );
}

#[test]
fn while_loop_body_mismatch_is_caught() {
    let source =
        include_str!("fixtures/expression-program-checking/while_loop_body_mismatch_is_caught.ts");
    let diagnostics = check(source, "while_loop_body_mismatch_is_caught.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert_eq!(diagnostics[0].severity, tsox::Severity::Error);
    assert!(
        diagnostics[0].message.contains("not assignable"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn while_loop_body_checks_clean() {
    let source =
        include_str!("fixtures/expression-program-checking/while_loop_body_checks_clean.ts");
    let diagnostics = check(source, "while_loop_body_checks_clean.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn for_loop_init_variable_is_registered_and_body_mismatch_is_caught() {
    let source =
        include_str!("fixtures/expression-program-checking/for_loop_body_mismatch_is_caught.ts");
    let diagnostics = check(source, "for_loop_body_mismatch_is_caught.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert!(
        diagnostics[0].message.contains("not assignable"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn for_loop_body_checks_clean() {
    let source = include_str!("fixtures/expression-program-checking/for_loop_body_checks_clean.ts");
    let diagnostics = check(source, "for_loop_body_checks_clean.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn switch_case_body_mismatch_is_caught() {
    let source =
        include_str!("fixtures/expression-program-checking/switch_case_body_mismatch_is_caught.ts");
    let diagnostics = check(source, "switch_case_body_mismatch_is_caught.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert!(
        diagnostics[0].message.contains("not assignable"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn switch_case_body_checks_clean_including_default() {
    let source =
        include_str!("fixtures/expression-program-checking/switch_case_body_checks_clean.ts");
    let diagnostics = check(source, "switch_case_body_checks_clean.ts");
    assert!(
        diagnostics.is_empty(),
        "expected no false positives, got: {diagnostics:?}"
    );
}

#[test]
fn array_indexing_with_numeric_literal_result_includes_undefined() {
    let source =
        include_str!("fixtures/expression-program-checking/array_indexing_numeric_literal.ts");
    let diagnostics = check(source, "array_indexing_numeric_literal.ts");
    // This used to assert no diagnostics, on the assumption that a literal
    // index (`numbers[0]`) is provably in bounds and so yields a plain
    // `number`. d91f583 removed that assumption: this checker does not track
    // array lengths any more than tsc does under noUncheckedIndexedAccess, so
    // a literal index includes `| undefined` exactly like a variable index
    // does (see array_indexing_with_dynamic_key_result_includes_undefined).
    // Declaring the return type as plain `number` is therefore a real
    // mismatch, the same one tsc reports on this line.
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic (the honest element | undefined vs number mismatch), got: {diagnostics:?}"
    );
    assert!(
        diagnostics[0].message.contains("is not assignable to type"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn array_indexing_with_numeric_literal_mismatch_is_caught() {
    let source = include_str!(
        "fixtures/expression-program-checking/array_indexing_numeric_literal_mismatch.ts"
    );
    let diagnostics = check(source, "array_indexing_numeric_literal_mismatch.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert!(
        diagnostics[0].message.contains("is not assignable to type"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn array_indexing_with_dynamic_key_result_includes_undefined() {
    let source = include_str!(
        "fixtures/expression-program-checking/array_indexing_dynamic_key_includes_undefined.ts"
    );
    let diagnostics = check(source, "array_indexing_dynamic_key_includes_undefined.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic (the honest element | undefined vs number mismatch), got: {diagnostics:?}"
    );
    assert!(
        diagnostics[0].message.contains("is not assignable to type"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn static_method_body_internal_error_is_caught() {
    let source =
        include_str!("fixtures/expression-program-checking/static_method_body_is_checked.ts");
    let diagnostics = check(source, "static_method_body_is_checked.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert!(
        diagnostics[0].message.contains("is not assignable to type"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn static_field_initializer_mismatch_is_caught() {
    let source = include_str!(
        "fixtures/expression-program-checking/static_field_initializer_mismatch_is_caught.ts"
    );
    let diagnostics = check(source, "static_field_initializer_mismatch_is_caught.ts");
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert!(
        diagnostics[0].message.contains("not assignable"),
        "got: {diagnostics:?}"
    );
}

#[test]
fn call_arity_mismatch_still_checks_argument_expressions() {
    let source = include_str!(
        "fixtures/expression-program-checking/call_arity_mismatch_still_checks_arguments.ts"
    );
    let diagnostics = check(source, "call_arity_mismatch_still_checks_arguments.ts");
    assert_eq!(
        diagnostics.len(),
        2,
        "expected the arity error and the internal argument error, got: {diagnostics:?}"
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message.contains("arguments, but got")),
        "got: {diagnostics:?}"
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message.contains("does not exist")),
        "got: {diagnostics:?}"
    );
}

#[test]
fn enum_object_is_assignable_to_a_structural_type_whatever_its_declaration_order() {
    let source = include_str!(
        "fixtures/expression-program-checking/enum_object_assignable_to_structural_type.ts"
    );
    let diagnostics = check(source, "enum_object_assignable_to_structural_type.ts");
    // Up, Down, Left, Right is not alphabetical. Before enum objects were built
    // with sorted properties, the merge-join misaligned on them and this reported
    // a false mismatch.
    assert!(
        diagnostics.is_empty(),
        "expected no diagnostics, got: {diagnostics:?}"
    );
}

#[test]
fn string_enum_object_is_assignable_to_a_structural_type_whatever_its_declaration_order() {
    let source = include_str!(
        "fixtures/expression-program-checking/string_enum_object_assignable_to_structural_type.ts"
    );
    let diagnostics = check(
        source,
        "string_enum_object_assignable_to_structural_type.ts",
    );
    // Pending, Active, Done is not alphabetical, and string members exercise the
    // literal-to-union path as well as the property ordering.
    assert!(
        diagnostics.is_empty(),
        "expected no diagnostics, got: {diagnostics:?}"
    );
}

#[test]
fn enum_object_missing_a_required_member_is_still_caught() {
    let source = include_str!(
        "fixtures/expression-program-checking/enum_object_missing_required_member_is_caught.ts"
    );
    let diagnostics = check(source, "enum_object_missing_required_member_is_caught.ts");
    // Sorting the enum object must not turn a real mismatch into a pass: `Medium`
    // is required by the target and the enum has no such member.
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert_eq!(diagnostics[0].severity, Severity::Error);
    assert_eq!(
        diagnostics[0].code,
        DiagnosticCode::MissingProperty,
        "got: {diagnostics:?}"
    );
    assert!(
        diagnostics[0]
            .message
            .contains("Property 'Medium' is missing"),
        "got: {diagnostics:?}"
    );
}
