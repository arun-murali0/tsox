use tsox::TypeChecker;

// Intersection types (LLD 1.13): what a user can write with `&`, and what is reported.
// Every fixture here was run through tsc 5.9.3 first; each one reports exactly what is
// asserted, with the same code and the same message.

fn check(source: &str, file_name: &str) -> Vec<tsox::Diagnostic> {
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

fn assert_one(source: &str, file_name: &str, fragment: &str) {
    let diagnostics = check(source, file_name);
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    assert!(
        diagnostics[0].message.contains(fragment),
        "expected a message containing {fragment:?}, got: {diagnostics:?}"
    );
}

// A property of an intersection comes from whichever member has it.
#[test]
fn intersection_has_the_properties_of_every_member() {
    let source = include_str!(
        "fixtures/intersection-types/intersection_has_the_properties_of_every_member.ts"
    );
    assert_clean(source, "intersection_has_the_properties_of_every_member.ts");
}

// An object that has every member's properties is assignable to the intersection and back to each member.
#[test]
fn an_object_with_every_property_is_assignable() {
    let source =
        include_str!("fixtures/intersection-types/an_object_with_every_property_is_assignable.ts");
    assert_clean(source, "an_object_with_every_property_is_assignable.ts");
}

// `A & B` and `B & A` are two ids but assignable both ways.
#[test]
fn member_order_does_not_matter_for_assignment() {
    let source =
        include_str!("fixtures/intersection-types/member_order_does_not_matter_for_assignment.ts");
    assert_clean(source, "member_order_does_not_matter_for_assignment.ts");
}

// A branded primitive still behaves as its primitive.
#[test]
fn branded_primitive_is_a_primitive() {
    let source = include_str!("fixtures/intersection-types/branded_primitive_is_a_primitive.ts");
    assert_clean(source, "branded_primitive_is_a_primitive.ts");
}

// An alias of an intersection flattens into a larger one.
#[test]
fn alias_of_an_intersection_can_be_extended() {
    let source =
        include_str!("fixtures/intersection-types/alias_of_an_intersection_can_be_extended.ts");
    assert_clean(source, "alias_of_an_intersection_can_be_extended.ts");
}

// An intersection of unions keeps only what the unions share.
#[test]
fn intersection_of_unions_distributes() {
    let source = include_str!("fixtures/intersection-types/intersection_of_unions_distributes.ts");
    assert_clean(source, "intersection_of_unions_distributes.ts");
}

// `A & {}` is `A`.
#[test]
fn empty_object_next_to_an_object_is_dropped() {
    let source =
        include_str!("fixtures/intersection-types/empty_object_next_to_an_object_is_dropped.ts");
    assert_clean(source, "empty_object_next_to_an_object_is_dropped.ts");
}

// A type parameter with extra members infers from the argument.
#[test]
fn generic_parameter_with_extra_members() {
    let source =
        include_str!("fixtures/intersection-types/generic_parameter_with_extra_members.ts");
    assert_clean(source, "generic_parameter_with_extra_members.ts");
}

// A call through an intersection of functions uses the first member that accepts it.
#[test]
fn first_matching_member_of_a_function_intersection_is_called() {
    let source = include_str!(
        "fixtures/intersection-types/first_matching_member_of_a_function_intersection_is_called.ts"
    );
    assert_clean(
        source,
        "first_matching_member_of_a_function_intersection_is_called.ts",
    );
}

// A property declared with two different types is `never`, assignable to both.
#[test]
fn conflicting_property_types_give_never() {
    let source =
        include_str!("fixtures/intersection-types/conflicting_property_types_give_never.ts");
    assert_clean(source, "conflicting_property_types_give_never.ts");
}

// An object missing one member's property is rejected.
#[test]
fn missing_property_of_an_intersection_is_reported() {
    let source = include_str!(
        "fixtures/intersection-types/missing_property_of_an_intersection_is_reported.ts"
    );
    assert_one(
        source,
        "missing_property_of_an_intersection_is_reported.ts",
        "not assignable to type 'A & B'",
    );
}

// A property no member has is reported.
#[test]
fn property_on_no_member_is_reported() {
    let source = include_str!("fixtures/intersection-types/property_on_no_member_is_reported.ts");
    assert_one(
        source,
        "property_on_no_member_is_reported.ts",
        "does not exist on type 'A & B'",
    );
}

// `string & number` is `never`.
#[test]
fn disjoint_primitives_are_never() {
    let source = include_str!("fixtures/intersection-types/disjoint_primitives_are_never.ts");
    assert_one(
        source,
        "disjoint_primitives_are_never.ts",
        "not assignable to type 'never'",
    );
}

// Members that disagree about a discriminant make `never`, which has no properties.
#[test]
fn discriminant_conflict_has_no_properties() {
    let source =
        include_str!("fixtures/intersection-types/discriminant_conflict_has_no_properties.ts");
    assert_one(
        source,
        "discriminant_conflict_has_no_properties.ts",
        "does not exist on type 'never'",
    );
}

// A name that does not exist inside an intersection is reported.
#[test]
fn unknown_name_in_an_intersection_is_reported() {
    let source =
        include_str!("fixtures/intersection-types/unknown_name_in_an_intersection_is_reported.ts");
    assert_one(
        source,
        "unknown_name_in_an_intersection_is_reported.ts",
        "Cannot find name 'Missing'",
    );
}

// A plain string is not a branded one.
#[test]
fn plain_string_is_not_a_branded_string() {
    let source =
        include_str!("fixtures/intersection-types/plain_string_is_not_a_branded_string.ts");
    assert_one(
        source,
        "plain_string_is_not_a_branded_string.ts",
        "not assignable to type 'UserId'",
    );
}

// An intersection is not assignable to a type none of its members satisfy.
#[test]
fn intersection_is_not_assignable_to_an_unrelated_type() {
    let source = include_str!(
        "fixtures/intersection-types/intersection_is_not_assignable_to_an_unrelated_type.ts"
    );
    assert_one(
        source,
        "intersection_is_not_assignable_to_an_unrelated_type.ts",
        "Type 'A & B' is not assignable to type 'number'",
    );
}
