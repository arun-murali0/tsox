//! The worker against real projects on disk.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use tower_lsp_server::ls_types::DiagnosticSeverity;
use tsox_lsp::worker::{Command, Event, FileUpdate, Worker};

const BAD: &str = "export const n: number = 'x';\n";
const GOOD: &str = "export const n: number = 1;\n";

struct Fixture {
    dir: tempfile::TempDir,
    root: PathBuf,
}

fn project(files: &[(&str, &str)]) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    std::fs::write(
        root.join("tsconfig.json"),
        r#"{ "compilerOptions": { "strict": true }, "include": ["*.ts"] }"#,
    )
    .unwrap();
    for (name, text) in files {
        std::fs::write(root.join(name), text).unwrap();
    }
    Fixture { dir, root }
}

fn buffer(path: &Path, version: i32, text: &str) -> Command {
    Command::Buffer {
        path: path.to_path_buf(),
        version,
        text: Arc::from(text),
    }
}

/// Everything the worker reports, until it stops. Shutting it down closes the channel, so
/// this sees every event and no more.
fn finish(worker: Worker, mut events: UnboundedReceiver<Event>) -> Vec<Event> {
    worker.shutdown();
    let mut all = Vec::new();
    while let Ok(event) = events.try_recv() {
        all.push(event);
    }
    all
}

fn next_diagnostics(events: &mut UnboundedReceiver<Event>) -> Vec<FileUpdate> {
    match events.blocking_recv().expect("the worker stopped early") {
        Event::Diagnostics { files, .. } => files,
        Event::Error(message) => panic!("worker error: {message}"),
    }
}

fn updates(events: &[Event]) -> Vec<&FileUpdate> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::Diagnostics { files, .. } => Some(files.iter()),
            Event::Error(_) => None,
        })
        .flatten()
        .collect()
}

#[test]
fn an_open_file_with_an_error_is_reported_at_its_version() {
    let fx = project(&[("a.ts", GOOD)]);
    let a = fx.root.join("a.ts");
    let (tx, mut rx) = unbounded_channel();
    let worker = Worker::spawn(Some(fx.root.clone()), tx).unwrap();

    worker.send(buffer(&a, 3, BAD));
    let files = next_diagnostics(&mut rx);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, a);
    assert_eq!(files[0].version, 3);
    assert_eq!(files[0].diagnostics.len(), 1);
    assert_eq!(
        files[0].diagnostics[0].severity,
        Some(DiagnosticSeverity::ERROR)
    );
    assert_eq!(files[0].diagnostics[0].range.start.line, 0);
    worker.shutdown();
    drop(fx.dir);
}

#[test]
fn fixing_the_file_clears_its_diagnostics() {
    let fx = project(&[("a.ts", GOOD)]);
    let a = fx.root.join("a.ts");
    let (tx, mut rx) = unbounded_channel();
    let worker = Worker::spawn(Some(fx.root.clone()), tx).unwrap();

    worker.send(buffer(&a, 1, BAD));
    assert_eq!(next_diagnostics(&mut rx)[0].diagnostics.len(), 1);
    worker.send(buffer(&a, 2, GOOD));
    let files = next_diagnostics(&mut rx);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].version, 2);
    assert!(
        files[0].diagnostics.is_empty(),
        "the client must be told to clear"
    );
    worker.shutdown();
}

#[test]
fn an_edit_that_changes_nothing_reported_sends_nothing() {
    let fx = project(&[("a.ts", GOOD)]);
    let a = fx.root.join("a.ts");
    let (tx, mut rx) = unbounded_channel();
    let worker = Worker::spawn(Some(fx.root.clone()), tx).unwrap();

    worker.send(buffer(&a, 1, BAD));
    assert_eq!(next_diagnostics(&mut rx).len(), 1);
    // The same error on the same line: the client already has it. The check still runs and
    // reports, with nothing to update; waiting for it is what makes this a real test.
    worker.send(buffer(&a, 2, BAD));
    assert!(next_diagnostics(&mut rx).is_empty());
    let events = finish(worker, rx);
    assert!(updates(&events).is_empty(), "got {events:?}");
}

#[test]
fn a_clean_file_that_stays_clean_sends_nothing() {
    let fx = project(&[("a.ts", GOOD)]);
    let a = fx.root.join("a.ts");
    let (tx, mut rx) = unbounded_channel();
    let worker = Worker::spawn(Some(fx.root.clone()), tx).unwrap();
    worker.send(buffer(&a, 1, GOOD));
    assert!(next_diagnostics(&mut rx).is_empty());
    let events = finish(worker, rx);
    assert!(updates(&events).is_empty(), "got {events:?}");
}

#[test]
fn a_change_in_one_file_reaches_the_file_that_imports_it() {
    let fx = project(&[
        ("a.ts", "export const value: number = 1;\n"),
        (
            "b.ts",
            "import { value } from './a';\nexport const s: number = value;\n",
        ),
    ]);
    let (a, b) = (fx.root.join("a.ts"), fx.root.join("b.ts"));
    let (tx, mut rx) = unbounded_channel();
    let worker = Worker::spawn(Some(fx.root.clone()), tx).unwrap();

    worker.send(buffer(&a, 1, "export const value: number = 1;\n"));
    worker.send(buffer(
        &b,
        1,
        "import { value } from './a';\nexport const s: number = value;\n",
    ));
    // Both are clean, so whatever is reported is empty of errors.
    let mut seen = Vec::new();
    seen.extend(next_diagnostics(&mut rx));
    assert!(seen.iter().all(|f| f.diagnostics.is_empty()));

    worker.send(buffer(&a, 2, "export const value: string = 'x';\n"));
    let files = next_diagnostics(&mut rx);
    let for_b = files
        .iter()
        .find(|f| f.path == b)
        .expect("b.ts is now wrong");
    assert_eq!(for_b.diagnostics.len(), 1);
    assert_eq!(for_b.diagnostics[0].range.start.line, 1);
    worker.shutdown();
}

#[test]
fn a_burst_of_edits_ends_with_the_last_one() {
    let fx = project(&[("a.ts", GOOD)]);
    let a = fx.root.join("a.ts");
    let (tx, mut rx) = unbounded_channel();
    let worker = Worker::spawn(Some(fx.root.clone()), tx).unwrap();

    for version in 1..=40 {
        let text = if version % 2 == 0 { GOOD } else { BAD };
        worker.send(buffer(&a, version, text));
    }
    worker.send(buffer(&a, 41, BAD));

    // Wait until version 41 is reported, however many checks the burst was folded into.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let mut seen: Vec<FileUpdate> = Vec::new();
    while !seen.iter().any(|u| u.version == 41) {
        assert!(
            std::time::Instant::now() < deadline,
            "version 41 never reported"
        );
        match rx.try_recv() {
            Ok(Event::Diagnostics { files, .. }) => seen.extend(files),
            Ok(Event::Error(message)) => panic!("worker error: {message}"),
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(5)),
        }
    }
    worker.shutdown();
    let last = seen.last().unwrap();
    assert_eq!(last.version, 41);
    assert_eq!(last.diagnostics.len(), 1);
    // A version is never reported after a newer one.
    assert!(
        seen.windows(2)
            .all(|pair| pair[0].version <= pair[1].version)
    );
}

#[test]
fn closing_a_document_forgets_what_was_sent_for_it() {
    let fx = project(&[("a.ts", BAD)]);
    let a = fx.root.join("a.ts");
    let (tx, mut rx) = unbounded_channel();
    let worker = Worker::spawn(Some(fx.root.clone()), tx).unwrap();

    worker.send(buffer(&a, 1, BAD));
    assert_eq!(next_diagnostics(&mut rx).len(), 1);
    worker.send(Command::Close { path: a.clone() });
    // Closing reports nothing; the server clears the client's list itself.
    worker.send(buffer(&a, 1, BAD));
    let files = next_diagnostics(&mut rx);
    assert_eq!(files.len(), 1, "a reopened document is reported again");
    assert_eq!(files[0].diagnostics.len(), 1);
    worker.shutdown();
}

#[test]
fn a_folder_without_tsconfig_checks_the_open_documents_alone() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let a = root.join("a.ts");
    let b = root.join("b.ts");
    std::fs::write(&a, GOOD).unwrap();
    std::fs::write(&b, GOOD).unwrap();
    let (tx, mut rx) = unbounded_channel();
    let worker = Worker::spawn(None, tx).unwrap();

    worker.send(buffer(&a, 1, BAD));
    let files = next_diagnostics(&mut rx);
    assert_eq!(files[0].path, a);
    assert_eq!(files[0].diagnostics.len(), 1);

    // A second document the first project did not have: the project is opened again.
    worker.send(buffer(&b, 1, BAD));
    let files = next_diagnostics(&mut rx);
    assert!(
        files
            .iter()
            .any(|f| f.path == b && f.diagnostics.len() == 1)
    );
    worker.shutdown();
}

#[test]
fn a_document_that_is_not_a_file_gets_no_project_and_no_error() {
    let (tx, rx) = unbounded_channel();
    let worker = Worker::spawn(None, tx).unwrap();
    worker.send(buffer(Path::new("/no/such/place/untitled.ts"), 1, BAD));
    let events = finish(worker, rx);
    assert!(events.is_empty(), "got {events:?}");
}

#[test]
fn a_file_added_to_the_project_is_found_after_files_changed() {
    let fx = project(&[("a.ts", GOOD)]);
    let a = fx.root.join("a.ts");
    let (tx, mut rx) = unbounded_channel();
    let worker = Worker::spawn(Some(fx.root.clone()), tx).unwrap();
    worker.send(buffer(&a, 1, GOOD));
    let _ = next_diagnostics(&mut rx);

    let c = fx.root.join("c.ts");
    std::fs::write(&c, GOOD).unwrap();
    worker.send(Command::FilesChanged);
    let _ = next_diagnostics(&mut rx);
    // c.ts is now in the project, so an open buffer of it with an error is reported.
    worker.send(buffer(&c, 1, BAD));
    let files = next_diagnostics(&mut rx);
    assert!(
        files
            .iter()
            .any(|f| f.path == c && f.diagnostics.len() == 1)
    );
    worker.shutdown();
}

#[test]
fn dropping_the_worker_stops_the_thread() {
    let (tx, mut rx) = unbounded_channel();
    let worker = Worker::spawn(None, tx).unwrap();
    drop(worker);
    assert!(
        rx.blocking_recv().is_none(),
        "the sender goes with the thread"
    );
}

#[test]
fn a_forgotten_document_is_reported_again() {
    let fx = project(&[("a.ts", GOOD)]);
    let a = fx.root.join("a.ts");
    let (tx, mut rx) = unbounded_channel();
    let worker = Worker::spawn(Some(fx.root.clone()), tx).unwrap();

    worker.send(buffer(&a, 1, BAD));
    assert_eq!(next_diagnostics(&mut rx).len(), 1);
    worker.send(Command::Forget { path: a.clone() });
    let files = next_diagnostics(&mut rx);
    assert_eq!(files.len(), 1, "the same diagnostics, sent again");
    assert_eq!(files[0].diagnostics.len(), 1);
    worker.shutdown();
}

#[test]
fn the_project_files_are_the_users_own() {
    let fx = project(&[("a.ts", GOOD), ("b.ts", GOOD)]);
    let (a, b) = (fx.root.join("a.ts"), fx.root.join("b.ts"));
    let (tx, mut rx) = unbounded_channel();
    let worker = Worker::spawn(Some(fx.root.clone()), tx).unwrap();
    let queries = worker.queries();

    // Before anything is open there is no project.
    assert_eq!(queries.project_files(), Some(Vec::new()));

    worker.send(buffer(&a, 1, GOOD));
    let _ = next_diagnostics(&mut rx);
    let mut files = queries.project_files().unwrap();
    files.sort();
    assert_eq!(files, vec![a, b], "no library files, no config");
    worker.shutdown();
}

#[test]
fn a_question_does_not_cancel_or_repeat_a_check() {
    let fx = project(&[("a.ts", GOOD)]);
    let a = fx.root.join("a.ts");
    let (tx, mut rx) = unbounded_channel();
    let worker = Worker::spawn(Some(fx.root.clone()), tx).unwrap();
    let queries = worker.queries();

    worker.send(buffer(&a, 1, BAD));
    assert_eq!(next_diagnostics(&mut rx).len(), 1);
    for _ in 0..5 {
        assert!(queries.project_files().is_some());
    }
    let events = finish(worker, rx);
    assert!(events.is_empty(), "questions caused no checks: {events:?}");
}

#[test]
fn asking_a_worker_that_has_stopped_gives_none() {
    let (tx, _rx) = unbounded_channel();
    let worker = Worker::spawn(None, tx).unwrap();
    let queries = worker.queries();
    worker.shutdown();
    assert_eq!(queries.project_files(), None);
}

#[test]
fn the_type_at_an_offset_comes_from_the_checked_project() {
    let fx = project(&[(
        "a.ts",
        "export const n: number = 1;\nexport const s = 'x';\n",
    )]);
    let a = fx.root.join("a.ts");
    let (tx, mut rx) = unbounded_channel();
    let worker = Worker::spawn(Some(fx.root.clone()), tx).unwrap();
    let queries = worker.queries();

    // Nothing is checked yet.
    assert_eq!(queries.type_at(&a, 20, None), None);

    let text = "export const n: number = 1;\nexport const s = 'x';\nlet w = s.length;\n";
    worker.send(buffer(&a, 1, text));
    let _ = next_diagnostics(&mut rx);
    let at = |needle: &str| text.find(needle).unwrap() as u32;
    assert_eq!(
        queries.type_at(&a, at("w ="), Some(1)).unwrap().text,
        "number"
    );
    assert_eq!(
        queries.type_at(&a, at("s.length"), Some(1)).unwrap().text,
        "\"x\""
    );
    assert_eq!(queries.type_at(&a, 0, Some(1)), None, "a keyword");
    worker.shutdown();
}

#[test]
fn a_question_asked_during_a_burst_of_edits_is_answered_from_the_last_one() {
    let fx = project(&[("a.ts", GOOD)]);
    let a = fx.root.join("a.ts");
    let (tx, mut rx) = unbounded_channel();
    let worker = Worker::spawn(Some(fx.root.clone()), tx).unwrap();
    let queries = worker.queries();
    worker.send(buffer(&a, 1, "let v = 1;\n"));
    let _ = next_diagnostics(&mut rx);

    let asker = {
        let (queries, a) = (queries.clone(), a.clone());
        std::thread::spawn(move || queries.type_at(&a, 4, None))
    };
    for version in 2..30 {
        let text = if version % 2 == 0 {
            "let v = 'a';\n"
        } else {
            "let v = 1;\n"
        };
        worker.send(buffer(&a, version, text));
    }
    worker.send(buffer(&a, 30, "let v = true;\n"));
    // Whenever the question was answered, it was from a finished check of some version; and
    // asked again once the burst is over, it is the last version.
    let _ = asker.join().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert_eq!(queries.type_at(&a, 4, None).unwrap().text, "boolean");
    worker.shutdown();
}

#[test]
fn a_type_asked_of_an_older_version_is_not_answered_from_a_newer_one() {
    let fx = project(&[("a.ts", GOOD)]);
    let a = fx.root.join("a.ts");
    let (tx, mut rx) = unbounded_channel();
    let worker = Worker::spawn(Some(fx.root.clone()), tx).unwrap();
    let queries = worker.queries();
    worker.send(buffer(&a, 1, "let v = 1;\n"));
    let _ = next_diagnostics(&mut rx);
    assert_eq!(queries.type_at(&a, 4, Some(1)).unwrap().text, "number");

    // The document moves on to version 2 before a question about version 1 is answered.
    worker.send(buffer(&a, 2, "let v = 'text';\n"));
    assert_eq!(
        queries.type_at(&a, 4, Some(1)),
        None,
        "the offset means version 1's text"
    );
    assert_eq!(queries.type_at(&a, 4, Some(2)).unwrap().text, "string");
    worker.shutdown();
}
