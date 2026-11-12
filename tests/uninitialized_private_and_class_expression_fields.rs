use tsox::{Severity, TypeChecker};

// A `#private` field and a field of a class expression are held to the same rule as a plain
// field of a class declaration: no initializer and no definite assignment is TS2564. Both
// shapes also draw a "not yet checked" warning, which the fixture harness would count, so
// they live here and look at errors only. Each case was compared with tsc 6.0.3.

fn errors(source: &str) -> Vec<(usize, String)> {
    let result = TypeChecker::new()
        .check_source(source, "case.ts")
        .expect("should parse");
    result
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| {
            let line = source[..d.start as usize].matches('\n').count() + 1;
            (line, d.message.clone())
        })
        .collect()
}

#[test]
fn a_private_field_without_an_initializer_is_reported_with_its_hash() {
    let found = errors("class A {\n  #name: string;\n  #count: number = 0;\n}\n");
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, 2);
    assert!(found[0].1.contains("'#name'"), "{found:?}");
}

#[test]
fn a_private_field_assigned_in_the_constructor_is_not_reported() {
    let found = errors(
        "class A {\n  #a: number;\n  #b?: number;\n  #c: string | undefined;\n  constructor() { this.#a = 1; }\n}\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_class_expression_field_without_an_initializer_is_reported() {
    let found = errors(
        "const E = class {\n  prop: number;\n  ok: number = 1;\n  opt?: string;\n};\nconst N = class Inner {\n  label: string;\n  constructor() { this.label = 'x'; }\n  other: boolean;\n};\n",
    );
    let lines: Vec<usize> = found.iter().map(|(line, _)| *line).collect();
    assert_eq!(lines, vec![2, 9], "{found:?}");
}
