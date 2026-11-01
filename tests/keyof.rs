use tsox::{FileId, Severity, UnitContext, Worker};

fn errors(source: &str) -> Vec<(Option<u32>, String)> {
    let ctx = UnitContext::new(FileId::ROOT, "keyof.ts", source);
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
fn keyof_an_object_type_is_the_union_of_its_property_names() {
    let found = errors(
        "type Point = { x: number; y: number };\n\
         const ok: keyof Point = 'x';\n\
         const also: keyof Point = 'y';\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn keyof_rejects_a_name_that_is_not_a_property() {
    let found = errors(
        "type Point = { x: number; y: number };\n\
         const bad: keyof Point = 'z';\n",
    );
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn keyof_an_interface_works_like_keyof_its_literal() {
    let found = errors(
        "interface User { id: number; name: string }\n\
         const ok: keyof User = 'name';\n\
         const bad: keyof User = 'age';\n",
    );
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn keyof_an_empty_object_is_never() {
    let found = errors(
        "type Empty = {};\n\
         const bad: keyof Empty = 'x';\n",
    );
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn keyof_an_intersection_is_the_union_of_both_sides_keys() {
    let found = errors(
        "type A = { a: 1 };\n\
         type B = { b: 2 };\n\
         const one: keyof (A & B) = 'a';\n\
         const two: keyof (A & B) = 'b';\n\
         const bad: keyof (A & B) = 'c';\n",
    );
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn keyof_a_union_is_the_keys_every_member_has() {
    let found = errors(
        "type A = { shared: 1; onlyA: 1 };\n\
         type B = { shared: 2; onlyB: 2 };\n\
         const ok: keyof (A | B) = 'shared';\n\
         const bad: keyof (A | B) = 'onlyA';\n",
    );
    assert_eq!(found.len(), 1, "{found:?}");
}
