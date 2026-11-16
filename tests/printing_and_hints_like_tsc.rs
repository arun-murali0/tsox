use tsox::{Severity, TypeChecker};

// Messages as tsc words them, each compared with tsc 6.0.3.

fn messages(source: &str) -> Vec<String> {
    TypeChecker::new()
        .check_source(source, "case.ts")
        .expect("should be checked")
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.message.clone())
        .collect()
}

#[test]
fn a_rest_parameter_is_written_with_the_dots_before_its_name() {
    let found = messages("declare var g: (a: number, ...rest: string[]) => void;\ng.fn;\n");
    assert!(
        found.iter().any(|m| m
            == "Property 'fn' does not exist on type '(a: number, ...rest: string[]) => void'."),
        "{found:?}"
    );
}

#[test]
fn exports_is_a_plain_unknown_name_not_a_node_types_hint() {
    let found = messages("exports.x = 1;\nrequire(\"a\");\n");
    assert!(
        found.iter().any(|m| m == "Cannot find name 'exports'."),
        "{found:?}"
    );
    assert!(
        found
            .iter()
            .any(|m| m.starts_with("Cannot find name 'require'. Do you need to install")),
        "{found:?}"
    );
}
