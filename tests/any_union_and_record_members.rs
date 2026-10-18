use tsox::{FileId, Severity, UnitContext, Worker};

// Two diagnostics that fired where tsc is silent, found in parts 05 and 06 of the
// TypeScript corpus:
//   TS2488  `const [, a] = x || []` with `x: any`: the union is `never[] | any`, which is `any`
//   TS2349  `rec.hasOwnProperty(k)` on a Record: the method, not the Record's value type

fn tsc_codes(source: &str) -> Vec<u32> {
    let ctx = UnitContext::new(FileId::ROOT, "a.ts", source);
    Worker::new()
        .check_unit(&ctx)
        .unwrap()
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .filter_map(|d| d.code.tsc_code())
        .collect()
}

// ---- TS2488 ----

#[test]
fn destructuring_an_any_or_empty_array_is_not_reported() {
    let found = tsc_codes("declare const x: any;\nconst [, a] = x || [];\n");
    assert!(!found.contains(&2488), "{found:?}");
}

#[test]
fn destructuring_with_a_default_over_an_any_or_empty_array_is_not_reported() {
    let found = tsc_codes("declare const x: any;\nconst [, a = \"\"] = x || [];\n");
    assert!(!found.contains(&2488), "{found:?}");
}

#[test]
fn a_union_without_any_is_still_reported() {
    let found = tsc_codes("declare const y: number | undefined;\nconst [p] = y || [];\n");
    assert!(found.contains(&2488), "{found:?}");
}

#[test]
fn a_plain_non_array_is_still_reported() {
    let found = tsc_codes("declare const n: number;\nconst [a] = n;\n");
    assert_eq!(found, vec![2488]);
}

// ---- TS2349 ----

#[test]
fn has_own_property_on_a_record_is_the_method() {
    let found = tsc_codes(
        "declare const o: Record<string, string>;\n\
         if (o.hasOwnProperty(\"k\")) { }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn has_own_property_on_a_record_returns_boolean_not_the_value_type() {
    let found = tsc_codes(
        "declare const o: Record<string, string>;\n\
         const n: number = o.hasOwnProperty(\"k\");\n",
    );
    assert_eq!(found, vec![2322]);
}

#[test]
fn other_keys_of_a_record_are_still_the_value_type() {
    let found = tsc_codes(
        "declare const o: Record<string, string>;\n\
         const n: number = o.other;\n",
    );
    assert_eq!(found, vec![2322]);
}

#[test]
fn to_string_on_a_record_is_the_method() {
    let found = tsc_codes(
        "declare const o: Record<string, number>;\n\
         const s: string = o.toString();\n",
    );
    assert!(found.is_empty(), "{found:?}");
}
