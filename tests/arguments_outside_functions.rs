use tsox::{Severity, TypeChecker};

// `arguments` belongs to a function that is not an arrow. Anywhere else it names nothing, and
// tsc says so (TS2304). Compared with tsc 6.0.3.

fn lines_of_missing_arguments(source: &str) -> Vec<usize> {
    let result = TypeChecker::new()
        .check_source(source, "case.ts")
        .expect("should parse");
    result
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error && d.message == "Cannot find name 'arguments'.")
        .map(|d| source[..d.start as usize].matches('\n').count() + 1)
        .collect()
}

#[test]
fn top_level_arrow_and_interface_uses_are_reported() {
    let source = "var a = arguments;\nvar f = () => arguments;\ninterface I { m: typeof arguments; }\nexport {};\n";
    assert_eq!(lines_of_missing_arguments(source), vec![1, 2, 3]);
}

#[test]
fn functions_methods_accessors_and_arrows_inside_them_are_fine() {
    let source = "function g() { return () => arguments; }\nclass C { m() { return arguments; } constructor() { arguments; } get p() { return arguments; } }\nvar o = { m() { return arguments; }, get q() { return arguments; } };\nfunction h(p = arguments) { return p; }\nexport {};\n";
    assert!(lines_of_missing_arguments(source).is_empty());
}

#[test]
fn a_declared_arguments_is_not_missing() {
    assert!(
        lines_of_missing_arguments("var arguments = 1;\nvar k = arguments;\nexport {};\n")
            .is_empty()
    );
}
