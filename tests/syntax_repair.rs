use tsox::{Severity, TypeChecker};

// Files oxc refuses are repaired and re-parsed, and tsc's syntax code is reported at the
// original position. Compared with tsc 6.0.3.

fn error_starts(source: &str) -> Vec<u32> {
    let result = TypeChecker::new()
        .check_source(source, "case.ts")
        .expect("should be checked");
    let mut starts: Vec<u32> = result
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.start)
        .collect();
    starts.sort();
    starts
}

#[test]
fn index_signatures_without_a_type_report_each_line() {
    let source = "interface A {\n  [x: string];\n  [y: number];\n}\n";
    let got = error_starts(source);
    let second = source.find("[y").unwrap() as u32;
    assert_eq!(got.len(), 2, "{got:?}");
    assert!(got[0] < second && got[1] >= second, "{got:?}");
}

#[test]
fn element_access_without_argument_is_reported_in_place() {
    let source = "var a: any;\nvar b = a[];\n";
    let got = error_starts(source);
    assert!(
        got.iter().any(|s| *s >= source.find("b =").unwrap() as u32),
        "{got:?}"
    );
}
