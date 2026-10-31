use tsox::{FileId, Severity, UnitContext, Worker};

fn errors(source: &str) -> Vec<(Option<u32>, String)> {
    let ctx = UnitContext::new(FileId::ROOT, "conditional.ts", source);
    Worker::new()
        .check_unit(&ctx)
        .unwrap()
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| (d.code.tsc_code(), d.message.to_string()))
        .collect()
}

#[test]
fn conditional_true_and_false_branches_reduce_for_concrete_inputs() {
    let found = errors(
        "type IsString<T> = T extends string ? true : false;\n\
         const yes: true = null as unknown as IsString<'hello'>;\n\
         const no: false = null as unknown as IsString<42>;\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn distributive_conditional_maps_a_union_member_by_member() {
    let found = errors(
        "type ToArray<T> = T extends unknown ? T[] : never;\n\
         type Both = ToArray<string | number>;\n\
         const good: string[] | number[] = null as unknown as Both;\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn distributive_conditional_rejects_an_array_of_the_whole_union() {
    let found = errors(
        "type ToArray<T> = T extends unknown ? T[] : never;\n\
         type Both = ToArray<string | number>;\n\
         const bad: Both = null as unknown as (string | number)[];\n",
    );
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn exclude_style_conditional_filters_union_members() {
    let found = errors(
        "type ExcludeLocal<T, U> = T extends U ? never : T;\n\
         type OnlyB = ExcludeLocal<'a' | 'b', 'a'>;\n\
         const good: 'b' = null as unknown as OnlyB;\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn exclude_style_conditional_does_not_keep_filtered_member() {
    let found = errors(
        "type ExcludeLocal<T, U> = T extends U ? never : T;\n\
         type OnlyB = ExcludeLocal<'a' | 'b', 'a'>;\n\
         const bad: OnlyB = 'a';\n",
    );
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn infer_from_array_preserves_literal_element_type() {
    let found = errors(
        "type ElementOf<T> = T extends (infer U)[] ? U : never;\n\
         type Leaf = ElementOf<'leaf'[]>;\n\
         const good: 'leaf' = null as unknown as Leaf;\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn infer_from_nested_object_property() {
    let found = errors(
        "type ValueOf<T> = T extends { value: infer U } ? U : never;\n\
         type Value = ValueOf<{ value: number }>;\n\
         const good: number = null as unknown as Value;\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn constrained_infer_accepts_a_matching_literal() {
    let found = errors(
        "type StringValue<T> = T extends { value: infer U extends string } ? U : never;\n\
         type Value = StringValue<{ value: 'ok' }>;\n\
         const good: 'ok' = null as unknown as Value;\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn constrained_infer_uses_the_false_branch_when_constraint_fails() {
    let found = errors(
        "type StringValue<T> = T extends { value: infer U extends string } ? U : never;\n\
         type Value = StringValue<{ value: 123 }>;\n\
         const bad: Value = 123;\n",
    );
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn conditional_type_deferred_until_generic_call_is_instantiated() {
    let found = errors(
        "type Kind<T> = T extends string ? 'text' : 'other';\n\
         function classify<T>(value: T): Kind<T> { return null as unknown as Kind<T>; }\n\
         const result: 'text' = classify('message');\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn distributive_conditional_over_never_is_never() {
    let found = errors(
        "type ToArray<T> = T extends unknown ? T[] : never;\n\
         type Empty = ToArray<never>;\n\
         const bad: Empty = 123;\n",
    );
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn conditional_type_with_any_exposes_both_branches() {
    let found = errors(
        "type IsString<T> = T extends string ? 'yes' : 'no';\n\
         const result: 'yes' | 'no' = null as unknown as IsString<any>;\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn promise_infer_extracts_resolved_value() {
    let found = errors(
        "type AwaitedLocal<T> = T extends Promise<infer U> ? U : T;\n\
         type Value = AwaitedLocal<Promise<number>>;\n\
         const good: number = null as unknown as Value;\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn infer_parameters_do_not_leak_into_the_false_branch() {
    let found = errors(
        "type Select<U, T> = T extends { value: infer U } ? U : U;\n\
         type Fallback = Select<boolean, { other: 1 }>;\n\
         const good: boolean = null as unknown as Fallback;\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn repeated_infer_name_collects_candidates_from_both_properties() {
    let found = errors(
        "type BothValues<T> = T extends { left: infer U; right: infer U } ? U : never;\n\
         type Values = BothValues<{ left: 'left'; right: 'right' }>;\n\
         const good: 'left' | 'right' = null as unknown as Values;\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn promise_conditional_checks_the_wrapped_value_type() {
    let found = errors(
        "type IsPromiseNumber<T> = T extends Promise<number> ? true : false;\n\
         const bad: true = null as unknown as IsPromiseNumber<Promise<string>>;\n",
    );
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn record_infer_extracts_value_type_from_application_arguments() {
    let found = errors(
        "type RecordValue<T> = T extends Record<string, infer U> ? U : never;\n\
         type Value = RecordValue<Record<string, number>>;\n\
         const good: number = null as unknown as Value;\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn parenthesized_naked_parameter_still_distributes() {
    let found = errors(
        "type ToArray<T> = (T) extends unknown ? T[] : never;\n\
         type Both = ToArray<string | number>;\n\
         const good: string[] | number[] = null as unknown as Both;\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn partial_makes_every_property_optional() {
    let found = errors(
        "type Source = { name: string; count: number };\n\
         type Maybe = Partial<Source>;\n\
         const value: Maybe = {};\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn partial_preserves_property_value_types() {
    let found = errors(
        "type Source = { name: string; count: number };\n\
         type Maybe = Partial<Source>;\n\
         const value: Maybe = { name: 123 };\n",
    );
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn required_removes_optional_property_modifier() {
    let found = errors(
        "type Source = { name?: string };\n\
         type Complete = Required<Source>;\n\
         const value: Complete = {};\n",
    );
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn pick_keeps_only_selected_properties() {
    let found = errors(
        "type Source = { name: string; count: number; active: boolean };\n\
         type Small = Pick<Source, 'name' | 'active'>;\n\
         const value: Small = { name: 'n', active: true };\n\
         const name: string = value.name;\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn omit_removes_selected_properties() {
    let found = errors(
        "type Source = { name: string; count: number };\n\
         type WithoutCount = Omit<Source, 'count'>;\n\
         const value: WithoutCount = { name: 'n' };\n\
         const count: number = value.count;\n",
    );
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn pick_rejects_a_missing_selected_key_by_not_fabricating_it() {
    let found = errors(
        "type Source = { name: string };\n\
         type Selected = Pick<Source, 'missing'>;\n\
         const value: Selected = { missing: 'x' };\n",
    );
    assert!(!found.is_empty(), "{found:?}");
}

#[test]
fn infer_from_function_return_type() {
    let found = errors(
        "type ReturnOf<T> = T extends () => infer U ? U : never;\n\
         type Value = ReturnOf<() => 'result'>;\n\
         const good: 'result' = null as unknown as Value;\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn repeated_covariant_infer_candidates_form_a_union() {
    let found = errors(
        "type ReturnOfBoth<T> = T extends { left: () => infer U; right: () => infer U } ? U : never;\n\
         type Value = ReturnOfBoth<{ left: () => 'left'; right: () => 'right' }>;\n\
         const good: 'left' | 'right' = null as unknown as Value;\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn repeated_contravariant_infer_candidates_form_an_intersection() {
    let found = errors(
        "type ArgumentOfBoth<T> = T extends { left: (value: infer U) => void; right: (value: infer U) => void } ? U : never;\n\
         type Value = ArgumentOfBoth<{ left: (value: { left: string }) => void; right: (value: { right: number }) => void }>;\n\
         const good: Value = { left: 'x', right: 1 };\n\
         const bad: Value = { left: 'x' };\n",
    );
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn covariant_infer_candidate_takes_precedence_over_contravariant_candidate() {
    let found = errors(
        "type Both<T> = T extends { produce: () => infer U; consume: (value: infer U) => void } ? U : never;\n\
         type Value = Both<{ produce: () => 'produced'; consume: (value: string) => void }>;\n\
         const good: 'produced' = null as unknown as Value;\n",
    );
    assert!(found.is_empty(), "{found:?}");
}
