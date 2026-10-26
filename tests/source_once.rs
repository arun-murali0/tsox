#![cfg(feature = "module-resolution")]

use std::fs;
use std::path::{Path, PathBuf};

use tsox::{FileOutcome, ModuleGraph, ModuleResolver, ProjectReport, check_project};

// Problem: discovery reads every file to find its imports, and the checker then read each
// file from the disk a second time. Two reads of one file can disagree if it changes in
// between, and each read costs a system call and a copy.
// Picked: discovery keeps the text it read, when it is valid UTF-8 and the file is scanned,
// and the checker uses that text. A file with no kept text is read by the checker, as it
// was before, so one that is not valid UTF-8 still fails to read.
// Now: the graph hands out the text it read, the checker checks that text and not whatever
// is on the disk later, and a file that is not valid UTF-8 is still a read failure.

fn project(test: &str, files: &[(&str, &[u8])]) -> (PathBuf, Vec<PathBuf>) {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("source-once-{test}"));
    let _ = fs::remove_dir_all(&root);
    let mut paths = Vec::new();
    for (name, bytes) in files {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        paths.push(path);
    }
    (root, paths)
}

fn build(root: &Path, paths: &[PathBuf]) -> ModuleGraph {
    ModuleGraph::build(paths, &ModuleResolver::new())
        .expect("the project builds")
        .with_root(root)
}

fn name_of(graph: &ModuleGraph, id: tsox::FileId) -> String {
    let path = graph.files().path(id).unwrap();
    path.file_name().unwrap().to_string_lossy().into_owned()
}

fn tsc_codes(graph: &ModuleGraph, report: &ProjectReport, file: &str) -> Vec<u32> {
    let mut codes = Vec::new();
    for entry in &report.files {
        if name_of(graph, entry.file_id) == file {
            if let FileOutcome::Checked(diagnostics) = &entry.outcome {
                codes.extend(diagnostics.iter().filter_map(|d| d.code.tsc_code()));
            }
        }
    }
    codes
}

#[test]
fn the_graph_hands_out_the_text_discovery_read() {
    let a = "import { b } from './b';\nexport const a = b;\n";
    let b = "export const b = 1;\n";
    let (root, paths) = project("text", &[("a.ts", a.as_bytes()), ("b.ts", b.as_bytes())]);
    let graph = build(&root, &paths);
    for (path, expected) in paths.iter().zip([a, b]) {
        let id = graph.files().id(path).expect("the file is in the graph");
        assert_eq!(graph.source(id), Some(expected), "{}", name_of(&graph, id));
    }
}

#[test]
fn the_checker_checks_the_text_discovery_read_and_not_the_disk_after_it() {
    // The file has a type error when the graph reads it and none when it is checked. The
    // report is for what the graph saw; `changed_files` is what says the file has moved.
    let (root, paths) = project(
        "disk-changes",
        &[("a.ts", b"export const n: number = 'text';\n")],
    );
    let graph = build(&root, &paths);
    fs::write(&paths[0], b"export const n: number = 1;\n").unwrap();

    let report = check_project(&graph);
    assert!(
        tsc_codes(&graph, &report, "a.ts").contains(&2322),
        "the error in the text discovery read must still be reported: {:?}",
        report.files.iter().map(|f| &f.outcome).collect::<Vec<_>>()
    );
    assert_eq!(graph.changed_files().len(), 1, "the edit is still seen");
}

#[test]
fn a_file_that_is_not_valid_utf8_keeps_no_text_and_is_still_a_read_failure() {
    let (root, paths) = project("not-utf8", &[("a.ts", &[0x63, 0x6f, 0xff, 0xfe, 0x3b])]);
    let graph = build(&root, &paths);
    let id = graph
        .files()
        .id(&paths[0])
        .expect("the file is in the graph");
    assert_eq!(graph.source(id), None);

    let report = check_project(&graph);
    let outcome = &report
        .files
        .iter()
        .find(|entry| entry.file_id == id)
        .expect("the file is reported")
        .outcome;
    assert!(
        matches!(outcome, FileOutcome::ReadFailed(text) if text.contains("valid UTF-8")),
        "{outcome:?}"
    );
}

#[test]
fn a_printed_graph_does_not_contain_the_source_text() {
    let marker = "a_name_that_would_show_up_if_the_text_were_printed";
    let source = format!("export const {marker} = 1;\n");
    let (root, paths) = project("debug", &[("a.ts", source.as_bytes())]);
    let graph = build(&root, &paths);
    let printed = format!("{graph:?}");
    assert!(!printed.contains(marker), "{printed}");
}

#[test]
fn text_read_once_gives_the_same_report_as_before_on_a_cycle_and_an_importer() {
    let (root, paths) = project(
        "cycle",
        &[
            ("a.ts", b"export * from './b';\nexport const a = 1;\n"),
            ("b.ts", b"export * from './a';\nexport const b = 2;\n"),
            (
                "use.ts",
                b"import { a, b } from './a';\nconst x: string = a;\nconst y: number = b;\n",
            ),
        ],
    );
    let graph = build(&root, &paths);
    let first = check_project(&graph);
    let second = check_project(&graph);
    // The same graph checked twice is the same report, and the importer's one real
    // mismatch is found.
    let lines = |report: &ProjectReport| -> Vec<String> {
        let mut all: Vec<String> = report
            .files
            .iter()
            .map(|file| format!("{}:{:?}", name_of(&graph, file.file_id), file.outcome))
            .collect();
        all.sort();
        all
    };
    assert_eq!(lines(&first), lines(&second));
    assert_eq!(tsc_codes(&graph, &first, "use.ts"), vec![2322]);
}
