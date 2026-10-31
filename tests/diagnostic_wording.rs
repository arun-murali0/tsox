#![cfg(feature = "module-resolution")]

use std::fs;
use std::path::Path;

use tsox::{FileOutcome, ModuleGraph, ModuleResolver, Severity, check_project};

// The text of diagnostics whose wording used to be tsox's own. Each is now tsc's: the same
// first line and the same reason under it, so a comparison against tsc sees no difference.

fn errors(test: &str, source: &str) -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("wording-{test}"));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let path = root.join("a.ts");
    fs::write(&path, source).unwrap();
    let graph = ModuleGraph::build(&[path], &ModuleResolver::new())
        .expect("the project builds")
        .with_root(root);
    let report = check_project(&graph);
    let mut found = Vec::new();
    for file in &report.files {
        let FileOutcome::Checked(diagnostics) = &file.outcome else {
            panic!("a file was not checked");
        };
        for d in diagnostics.iter().filter(|d| d.severity == Severity::Error) {
            found.push((format!("{:?}", d.code.tsc_code()), d.message.clone()));
        }
    }
    found
}

#[test]
fn calling_a_number_names_the_wrapper_type() {
    assert_eq!(
        errors("call-number", "const n: number = 5;\nn();\n"),
        [(
            "Some(2349)".to_string(),
            "This expression is not callable.\n  Type 'Number' has no call signatures.".to_string()
        )]
    );
}

#[test]
fn calling_a_string_literal_names_the_wrapper_type() {
    assert_eq!(
        errors("call-string", "const s = \"a\";\ns();\n")[0].1,
        "This expression is not callable.\n  Type 'String' has no call signatures."
    );
}

#[test]
fn calling_an_object_names_the_object_type() {
    assert_eq!(
        errors("call-object", "const o = { a: 1 };\no();\n")[0].1,
        "This expression is not callable.\n  Type '{ a: number; }' has no call signatures."
    );
}

#[test]
fn calling_a_union_says_no_constituent_is_callable() {
    assert_eq!(
        errors("call-union", "declare const u: string | number;\nu();\n")[0].1,
        "This expression is not callable.\n  No constituent of type 'string | number' is callable."
    );
}

#[test]
fn constructing_a_number_is_not_constructable() {
    assert_eq!(
        errors("new-number", "const n: number = 5;\nnew n();\n"),
        [(
            "Some(2351)".to_string(),
            "This expression is not constructable.\n  Type 'Number' has no construct signatures."
                .to_string()
        )]
    );
}

#[test]
fn a_destructuring_mismatch_is_reported_at_the_property_like_any_other() {
    // tsc has no wording of its own for a destructuring mismatch: it is the plain TS2322, on
    // the property whose value does not fit.
    assert_eq!(
        errors(
            "destructuring",
            "export const { a }: { a: string } = { a: 1 };\n"
        ),
        [(
            "Some(2322)".to_string(),
            "Type 'number' is not assignable to type 'string'.".to_string()
        )]
    );
}
