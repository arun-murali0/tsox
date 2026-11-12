use tsox::{Severity, TypeChecker};

// A literal `null` or `undefined` operand is TS18050 and is then treated as having the other
// operand's type, so the other operand can fail the operator's own check on the same line
// (tsc 6.0.3 reports both). The fixture harness takes one code per line, so the pairs are here.

fn messages(source: &str) -> Vec<String> {
    let result = TypeChecker::new()
        .check_source(source, "case.ts")
        .expect("should parse");
    let mut found: Vec<String> = result
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.message.clone())
        .collect();
    found.sort();
    found
}

const DECLS: &str = "declare var flag: boolean;\ndeclare var text: string;\n";

#[test]
fn the_other_side_still_fails_its_own_check() {
    let found = messages(&format!("{DECLS}const r = null ** flag;\n"));
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(found[0].starts_with("The right-hand side"), "{found:?}");
    assert!(found[1].contains("The value 'null'"), "{found:?}");

    let found = messages(&format!("{DECLS}const r = text - null;\n"));
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(found[0].starts_with("The left-hand side"), "{found:?}");
    assert!(found[1].contains("The value 'null'"), "{found:?}");
}

#[test]
fn two_literals_report_the_value_twice_and_nothing_else() {
    let found = messages("const r = null * undefined;\n");
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(
        found.iter().all(|m| m.contains("cannot be used here")),
        "{found:?}"
    );
}
