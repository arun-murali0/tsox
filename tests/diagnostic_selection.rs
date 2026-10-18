use tsox::{FileId, Severity, UnitContext, Worker};

// The code and head line tsc chooses for a mismatch, not only whether a line is reported.

fn errors(source: &str) -> Vec<(Option<u32>, String)> {
    let ctx = UnitContext::new(FileId::ROOT, "a.ts", source);
    Worker::new()
        .check_unit(&ctx)
        .unwrap()
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| (d.code.tsc_code(), d.message.to_string()))
        .collect()
}

// An element's missing property stays beneath the array-level mismatch, as in tsc.
#[test]
fn array_of_a_lacking_type_is_reported_at_the_array() {
    let found = errors(
        "interface I1 { a: number }\nclass C1 { a!: number; m!: string }\n\
         declare var i: I1[];\nvar c: C1[] = i;\n",
    );
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, Some(2322), "{found:?}");
    assert!(
        found[0]
            .1
            .starts_with("Type 'I1[]' is not assignable to type 'C1[]'."),
        "{found:?}"
    );
}

// A plain object still gets the missing-property head.
#[test]
fn object_lacking_a_property_is_reported_as_missing_property() {
    let found = errors("class C1 { a!: number; m!: string }\nvar o: C1 = { a: 1 };\n");
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, Some(2741), "{found:?}");
}

// A type used where a value is expected is TS2693, an unknown name is still TS2304.
#[test]
fn type_used_as_a_value_is_ts2693() {
    let found = errors(
        "interface I { a: number }\ntype A = { b: string };\nconst x = I;\nconst y = A;\nconst z = Iterator;\nconst w = Nope;\n",
    );
    let codes: Vec<_> = found.iter().map(|(code, _)| *code).collect();
    assert_eq!(
        codes,
        [Some(2693), Some(2693), Some(2693), Some(2304)],
        "{found:?}"
    );
}

// Members every value has are not reported as missing, while a member that does not exist is.
#[test]
fn members_every_value_has_are_found() {
    let found = errors(
        "enum E { A }\nconst a = (1).toString();\nconst b = E.toString();\n\
         const c = 's'.valueOf();\nconst d = [1].toString();\n\
         declare function f(): void;\nconst g = f.call(null);\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_member_that_does_not_exist_is_still_reported() {
    let found = errors("interface I { a: number }\ndeclare const i: I;\nconst h = i.nope;\n");
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, Some(2339), "{found:?}");
}

#[test]
fn a_const_assertion_is_not_an_unknown_name() {
    let found = errors(
        "enum E { A, B }\nconst a = 1 as const;\nconst b = E.B as const;\n\
         const c = ['a', 'b'] as const;\nconst d = { k: 1 } as const;\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn the_arguments_object_is_not_an_unknown_name() {
    let found = errors("function f() { return arguments.length; }\n");
    assert!(found.is_empty(), "{found:?}");
}

// The methods only a number has are found on a number and on a numeric literal, and are not
// found on other types; a non-number argument is rejected as tsc does.
#[test]
fn number_methods_are_found_on_numbers() {
    let found = errors(
        "declare const n: number;\nconst a = n.toFixed(2);\nconst b = (1.5).toExponential();\n\
         const c = n.toPrecision(3);\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn number_methods_are_not_found_on_a_string() {
    let found = errors("declare const s: string;\nconst a = s.toFixed(2);\n");
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, Some(2339), "{found:?}");
}

#[test]
fn number_method_argument_must_be_a_number() {
    let found = errors("declare const n: number;\nconst a = n.toFixed('2');\n");
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, Some(2345), "{found:?}");
}

// The string methods are found with their argument and result types; a wrong one is reported
// as in tsc, and a method a string does not have is still missing.
#[test]
fn string_methods_are_found() {
    let found = errors(
        "declare const s: string;\nconst a = s.startsWith('a');\nconst b = s.split(',').length;\n\
         const c = 'x'.padStart(3, '0');\nconst d = s.replace('a', 'b').trim().toUpperCase();\n\
         const e = s.slice(1);\nconst f = s.indexOf('a', 2) + 1;\nconst g = s.at(0);\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn string_method_argument_and_result_types_are_checked() {
    let found = errors(
        "declare const s: string;\nconst a: number = s.startsWith('a');\nconst b = s.startsWith(1);\n",
    );
    let codes: Vec<_> = found.iter().map(|(code, _)| *code).collect();
    assert_eq!(codes, [Some(2322), Some(2345)], "{found:?}");
}

#[test]
fn a_method_a_string_does_not_have_is_still_missing() {
    let found = errors("declare const s: string;\nconst a = s.nope();\nconst b = s.toFixed(1);\n");
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(
        found.iter().all(|(code, _)| *code == Some(2339)),
        "{found:?}"
    );
}
