//! Fixtures for the semantic features added on top of the generics work: index signatures,
//! `as const`, class and interface heritage, readonly, variadic rest parameters and
//! constructor types. Each fixture was compared with tsc (`scripts/compare-local.sh`).
//!
//! A line that ends in `// error TS<code>` must report exactly those tsc codes on that line;
//! a fixture with no such comment must check clean.

use std::fs;
use std::path::PathBuf;

use tsox::TypeChecker;

fn expected(source: &str) -> Vec<(usize, u32)> {
    let mut out = Vec::new();
    for (index, line) in source.lines().enumerate() {
        if let Some(rest) = line.split("// error TS").nth(1) {
            let code: u32 = rest.trim().parse().expect("error code after `// error TS`");
            out.push((index + 1, code));
        }
    }
    out
}

fn actual(source: &str, name: &str) -> Vec<(usize, u32)> {
    let checked = TypeChecker::new()
        .check_source(source, name)
        .expect("fixture should parse");
    let mut out: Vec<(usize, u32)> = checked
        .diagnostics
        .iter()
        .map(|d| {
            let line = source[..d.start as usize].matches('\n').count() + 1;
            (line, d.code.tsc_code().unwrap_or(0))
        })
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

#[test]
fn semantic_feature_fixtures_report_what_tsc_reports() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/semantic-features");
    let mut failures = Vec::new();
    for kind in ["pass", "fail"] {
        let mut files: Vec<_> = fs::read_dir(root.join(kind))
            .expect("fixture directory")
            .flatten()
            .map(|e| e.path())
            .collect();
        files.sort();
        for path in files {
            let source = fs::read_to_string(&path).expect("fixture source");
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let mut want = expected(&source);
            want.sort_unstable();
            want.dedup();
            let got = actual(&source, &name);
            if want != got {
                failures.push(format!("{kind}/{name}: expected {want:?}, got {got:?}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
