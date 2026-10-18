#![cfg(feature = "module-resolution")]

use std::fs;
use std::path::Path;

use ts_rust::{
    FileOutcome, ModuleGraph, ModuleResolver, ProjectReport, TypeChecker, check_project,
    with_deep_stack,
};

// Problem: the stack guard stops the checker's own recursion, but the parser and the semantic
// builder recurse on nested brackets and nothing in the checker can stop them, so a file
// nested far enough still aborted the run.
// Picked: count the nesting of `(`, `[` and `{` in one pass before the parse and refuse a
// file past 4,000 levels, which is a fifth of where the worst construct aborts a debug build.
// Cost: a count and not a parse, so a regular expression holding a quote or a backtick can
// skew it for a line; the limit is far above real code, so only a file built to be deep is
// affected.
// Now: bracket nesting cannot abort a run. Tests here pin the refusal, what does not count
// towards it, and that a file just inside it is still parsed and checked.

fn closer(opener: char) -> char {
    match opener {
        '(' => ')',
        '[' => ']',
        _ => '}',
    }
}

// Past the limit the parser is never reached, so this runs on the test thread, whose 2 MB
// stack could not survive parsing a nest this deep: reaching the assertion is part of it.
#[test]
fn a_file_whose_brackets_nest_past_the_limit_is_refused_before_it_is_parsed() {
    for opener in ['(', '[', '{'] {
        let source = format!(
            "export const x = {}1{};",
            opener.to_string().repeat(100_000),
            closer(opener).to_string().repeat(100_000)
        );
        let error = TypeChecker::new()
            .check_source(&source, "a.ts")
            .expect_err("a nest of 100,000 is refused");
        let message = error.to_string();
        assert!(
            message.contains("brackets nest 100000 levels deep"),
            "{message}"
        );
        assert!(message.contains("was not parsed"), "{message}");
    }
}

#[test]
fn brackets_in_strings_and_comments_do_not_count_towards_the_limit() {
    let filler = "(".repeat(50_000);
    let source = format!("// {filler}\nexport const s = \"{filler}\";\n/* {filler} */");
    let result = TypeChecker::new().check_source(&source, "a.ts");
    assert!(result.is_ok(), "{:?}", result.err());
}

// Inside the limit the file is parsed and checked. The parse needs more than a test thread's
// stack at this depth, which is what `with_deep_stack` is for.
#[test]
fn a_file_just_inside_the_limit_is_parsed_and_checked() {
    for opener in ['(', '['] {
        let source = format!(
            "export const x = {}1{};",
            opener.to_string().repeat(3_900),
            closer(opener).to_string().repeat(3_900)
        );
        let result = with_deep_stack(|| TypeChecker::new().check_source(&source, "a.ts"));
        assert!(result.is_ok(), "{opener}: {:?}", result.err());
    }
}

fn run(test: &str, files: &[(&str, String)]) -> ProjectReport {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("bracket-limit-{test}"));
    let _ = fs::remove_dir_all(&root);
    let mut paths = Vec::new();
    for (name, source) in files {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, source).unwrap();
        paths.push(path);
    }
    let graph = ModuleGraph::build(&paths, &ModuleResolver::new())
        .expect("the project builds")
        .with_root(root);
    check_project(&graph)
}

// In a project the file is refused twice over, in the walk that finds its imports and in the
// check. The project still completes, the other file is checked, and the refused one says why.
#[test]
fn a_file_nested_past_the_limit_is_reported_and_the_project_still_completes() {
    let nested = format!(
        "export const x = {}1{};",
        "(".repeat(100_000),
        ")".repeat(100_000)
    );
    let report = run(
        "refused",
        &[
            ("deep.ts", nested),
            ("fine.ts", "export const ok: number = 1;".to_string()),
        ],
    );
    assert_eq!(report.files.len(), 2);
    let mut refused = 0;
    for file in &report.files {
        match &file.outcome {
            FileOutcome::CheckFailed(error) => {
                assert!(error.to_string().contains("brackets nest"), "{error}");
                refused += 1;
            }
            FileOutcome::Checked(diagnostics) => assert!(diagnostics.is_empty(), "{diagnostics:?}"),
            other => panic!("unexpected outcome {other:?}"),
        }
    }
    assert_eq!(refused, 1);
}

// A file that nests deeper than the main thread or a test thread could parse, but inside the
// limit, is found and checked: the walk that reads imports runs on its own large stack.
#[test]
fn a_deep_file_inside_the_limit_is_found_and_checked_from_the_test_thread() {
    let nested = format!(
        "export const x = {}1{};",
        "(".repeat(3_900),
        ")".repeat(3_900)
    );
    let report = run("inside", &[("a.ts", nested)]);
    assert_eq!(report.files.len(), 1);
    assert!(
        matches!(report.files[0].outcome, FileOutcome::Checked(_)),
        "{:?}",
        report.files[0].outcome
    );
}
