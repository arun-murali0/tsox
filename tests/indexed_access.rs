use tsox::{FileId, Severity, UnitContext, Worker};

fn errors(source: &str) -> Vec<(Option<u32>, String)> {
    let ctx = UnitContext::new(FileId::ROOT, "indexed.ts", source);
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
fn a_property_name_picks_that_propertys_type() {
    let found = errors(
        "type Point = { x: number; y: string };\n\
         const ok: Point['x'] = 1;\n\
         const bad: Point['x'] = 'a';\n",
    );
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn an_interface_can_be_indexed_too() {
    let found = errors(
        "interface User { id: number; name: string }\n\
         const ok: User['name'] = 'ann';\n\
         const bad: User['id'] = 'ann';\n",
    );
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn a_union_of_names_gives_the_union_of_the_property_types() {
    let found = errors(
        "type Point = { x: number; y: string };\n\
         const one: Point['x' | 'y'] = 1;\n\
         const two: Point['x' | 'y'] = 'a';\n\
         const bad: Point['x' | 'y'] = true;\n",
    );
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn indexing_by_keyof_gives_every_property_type() {
    let found = errors(
        "type Point = { x: number; y: string };\n\
         const one: Point[keyof Point] = 1;\n\
         const two: Point[keyof Point] = 'a';\n\
         const bad: Point[keyof Point] = false;\n",
    );
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn an_optional_property_also_admits_undefined() {
    let found = errors(
        "type Opt = { a?: number };\n\
         const ok: Opt['a'] = undefined;\n\
         const also: Opt['a'] = 3;\n\
         const bad: Opt['a'] = 'no';\n",
    );
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn an_array_indexed_by_number_is_its_element_type() {
    let found = errors(
        "const ok: number[][number] = 1;\n\
         const bad: number[][number] = 'a';\n",
    );
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn a_tuple_indexed_by_position_is_that_element() {
    let found = errors(
        "type Pair = [string, number];\n\
         const first: Pair[0] = 'a';\n\
         const second: Pair[1] = 2;\n\
         const bad: Pair[0] = 2;\n",
    );
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn a_tuple_indexed_by_number_is_the_union_of_its_elements() {
    let found = errors(
        "type Pair = [string, number];\n\
         const one: Pair[number] = 'a';\n\
         const two: Pair[number] = 2;\n\
         const bad: Pair[number] = true;\n",
    );
    assert_eq!(found.len(), 1, "{found:?}");
}
