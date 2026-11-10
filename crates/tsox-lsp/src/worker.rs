//! The thread that owns the project.
//!
//! `IncrementalProject` takes `&mut self` and holds a Salsa database that is not `Sync`, so
//! it cannot be shared between async handlers. One dedicated thread owns it; the server
//! sends commands and receives events.
//!
//! Commands queue up while a check runs. Sending one cancels the check in progress (the
//! checker polls a `CancelToken`), and the worker then takes everything that queued, applies
//! it in order, and checks once. The queue and the token share one lock, so a command is
//! never left waiting behind a check that was started after it arrived.
//!
//! What comes back is only what changed: the diagnostics of open documents that differ from
//! what was last sent. A check that was cancelled reports nothing.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;

use tokio::sync::mpsc::UnboundedSender;
use tower_lsp_server::ls_types::{
    Diagnostic as LspDiagnostic, DiagnosticSeverity, Position, Range,
};
use tsox::{CancelToken, CheckStats, FileId, FileOutcome, IncrementalProject, TypeAtOffset};

use crate::convert::{self, SOURCE};
use crate::paths::canonical_or_same;

/// What the server asks of the worker. Paths are the keys `paths::uri_to_path` gives.
#[derive(Debug)]
pub enum Command {
    /// The editor opened or changed a document: this is its whole text now.
    Buffer {
        path: PathBuf,
        version: i32,
        text: Arc<str>,
    },
    /// The editor closed a document; the file on disk is the source again.
    Close { path: PathBuf },
    /// The server could not use what was last reported for a document (it had changed again
    /// by the time the report arrived): report it afresh at the next check, even if the
    /// diagnostics are what they were.
    Forget { path: PathBuf },
    /// Files changed on disk (a watcher fired, or a file was saved): look for added or
    /// removed files and for changed configuration.
    FilesChanged,
    /// Stop the thread.
    Shutdown,
}

/// The diagnostics of one open document, at the version of the text that was checked.
#[derive(Clone, Debug, PartialEq)]
pub struct FileUpdate {
    pub path: PathBuf,
    pub version: i32,
    pub diagnostics: Vec<LspDiagnostic>,
}

#[derive(Debug)]
pub enum Event {
    /// A check finished. `files` holds the open documents whose diagnostics differ from the
    /// last report; it may be empty.
    Diagnostics {
        files: Vec<FileUpdate>,
        stats: CheckStats,
    },
    /// The project could not be opened or checked. The worker keeps running and tries again
    /// on the next command.
    Error(String),
}

/// A question about the project, answered between checks. It never cancels one: asking
/// is not a change.
enum Query {
    ProjectFiles(std::sync::mpsc::Sender<Vec<PathBuf>>),
    TypeAt {
        path: PathBuf,
        offset: u32,
        /// The version of the document the offset is an offset into. The answer is only given
        /// if the worker holds that same version.
        version: Option<i32>,
        reply: std::sync::mpsc::Sender<Option<TypeAtOffset>>,
    },
}

struct State {
    commands: VecDeque<Command>,
    queries: VecDeque<Query>,
    token: CancelToken,
    /// The thread has stopped; a query put on the queue now would never be answered.
    closed: bool,
}

struct Shared {
    state: Mutex<State>,
    ready: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Waits for something to do and takes all of it. When there are commands, a fresh token
    /// is installed in the same step, under the lock: anything sent after this returns cancels
    /// the token returned. Queries alone leave the token as it is.
    fn take_batch(&self) -> (Vec<Command>, Vec<Query>, CancelToken) {
        let mut state = self.lock();
        while state.commands.is_empty() && state.queries.is_empty() {
            state = self.ready.wait(state).unwrap_or_else(|e| e.into_inner());
        }
        let commands: Vec<Command> = state.commands.drain(..).collect();
        let queries = state.queries.drain(..).collect();
        if !commands.is_empty() {
            state.token = CancelToken::new();
        }
        (commands, queries, state.token.clone())
    }

    /// Puts queries back at the front of the queue, for the next turn: they were not answered
    /// because the check they follow was overtaken by a newer command.
    fn requeue(&self, queries: Vec<Query>) {
        let mut state = self.lock();
        for query in queries.into_iter().rev() {
            state.queries.push_front(query);
        }
    }

    /// Marks the thread as stopped and drops the queries still waiting, which makes the
    /// callers that wait on them give up instead of waiting forever.
    fn close(&self) {
        let mut state = self.lock();
        state.closed = true;
        state.queries.clear();
    }
}

/// What a thread that asks questions of the worker holds. It does not stop the worker.
#[derive(Clone)]
pub struct Queries {
    shared: Arc<Shared>,
}

impl Queries {
    /// The files of the project that are the user's own: not the standard library, not
    /// declaration files of dependencies, not `node_modules`. Waits for a check in progress
    /// to finish. `None` when the worker has stopped. Empty before a project is open.
    pub fn project_files(&self) -> Option<Vec<PathBuf>> {
        let (reply, answer) = std::sync::mpsc::channel();
        {
            let mut state = self.shared.lock();
            if state.closed {
                return None;
            }
            state.queries.push_back(Query::ProjectFiles(reply));
            self.shared.ready.notify_one();
        }
        answer.recv().ok()
    }

    /// The type at `offset` of the file at `path`, as the project's last finished check sees
    /// it (see `IncrementalProject::type_at`). Waits for a check in progress, and for the
    /// next one if a newer change overtakes it. `None` when nothing typed is there, the file
    /// is not in the project, or the worker has stopped.
    ///
    /// `version` is the version of the document the offset was taken from. If the worker has
    /// moved on to a newer one by the time it answers, there is no answer, rather than a type
    /// from different text than the offset means.
    pub fn type_at(&self, path: &Path, offset: u32, version: Option<i32>) -> Option<TypeAtOffset> {
        let (reply, answer) = std::sync::mpsc::channel();
        {
            let mut state = self.shared.lock();
            if state.closed {
                return None;
            }
            state.queries.push_back(Query::TypeAt {
                path: path.to_path_buf(),
                offset,
                version,
                reply,
            });
            self.shared.ready.notify_one();
        }
        answer.recv().ok().flatten()
    }
}

/// The handle the server keeps. Dropping it stops the thread.
pub struct Worker {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    /// Starts the thread. `root` is the workspace folder, if the client gave one: a
    /// `tsconfig.json` in it defines the project.
    pub fn spawn(root: Option<PathBuf>, events: UnboundedSender<Event>) -> io::Result<Self> {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                commands: VecDeque::new(),
                queries: VecDeque::new(),
                token: CancelToken::new(),
                closed: false,
            }),
            ready: Condvar::new(),
        });
        let worker_shared = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("tsox-lsp-worker".to_owned())
            .spawn(move || {
                // However the thread ends, nobody is left waiting on a question.
                struct Close(Arc<Shared>);
                impl Drop for Close {
                    fn drop(&mut self) {
                        self.0.close();
                    }
                }
                let _close = Close(Arc::clone(&worker_shared));
                // The checker recurses; this gives it the stack it asks for.
                tsox::with_deep_stack(move || run(&worker_shared, root, events));
            })?;
        Ok(Self {
            shared,
            thread: Some(thread),
        })
    }

    /// A handle for asking the worker questions from another thread.
    pub fn queries(&self) -> Queries {
        Queries {
            shared: Arc::clone(&self.shared),
        }
    }

    /// Queues a command and cancels the check in progress, which it makes out of date.
    pub fn send(&self, command: Command) {
        let mut state = self.shared.lock();
        state.token.cancel();
        state.commands.push_back(command);
        self.shared.ready.notify_one();
    }

    /// Stops the thread and waits for it.
    pub fn shutdown(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        if let Some(thread) = self.thread.take() {
            self.send(Command::Shutdown);
            let _ = thread.join();
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop();
    }
}

/// How the project was opened, which decides when it has to be opened again.
enum Mode {
    /// From a `tsconfig.json`; the files it selects are the project.
    Config,
    /// From the open documents alone, because no `tsconfig.json` was found. A document that
    /// is not among these entries makes the project be opened again with it.
    Entries(BTreeSet<PathBuf>),
}

struct Engine {
    root: Option<PathBuf>,
    project: Option<(IncrementalProject, Mode)>,
    open: BTreeMap<PathBuf, (i32, Arc<str>)>,
    // What the client was last told for each open document.
    sent: HashMap<PathBuf, Vec<LspDiagnostic>>,
    rescan: bool,
    events: UnboundedSender<Event>,
}

fn run(shared: &Shared, root: Option<PathBuf>, events: UnboundedSender<Event>) {
    let mut engine = Engine {
        root,
        project: None,
        open: BTreeMap::new(),
        sent: HashMap::new(),
        rescan: false,
        events,
    };
    loop {
        let (batch, queries, token) = shared.take_batch();
        let changed = !batch.is_empty();
        for command in batch {
            match command {
                Command::Shutdown => return,
                command => engine.apply(command),
            }
        }
        if engine.events.is_closed() {
            return;
        }
        if changed {
            engine.check(&token);
            if token.is_cancelled() {
                // Overtaken: the project is not at rest. The questions wait for the check
                // of what overtook it.
                shared.requeue(queries);
                continue;
            }
        }
        for query in queries {
            engine.answer(query);
        }
    }
}

impl Engine {
    fn apply(&mut self, command: Command) {
        match command {
            Command::Buffer {
                path,
                version,
                text,
            } => {
                let path = canonical_or_same(&path);
                let outside = matches!(
                    &self.project,
                    Some((_, Mode::Entries(entries))) if !entries.contains(&path)
                );
                if outside {
                    // Not in the project the open documents made: open it again.
                    self.project = None;
                } else if let Some((project, _)) = &mut self.project {
                    project.set_buffer(&path, &text);
                }
                self.open.insert(path, (version, text));
            }
            Command::Close { path } => {
                let path = canonical_or_same(&path);
                self.open.remove(&path);
                self.sent.remove(&path);
                if let Some((project, _)) = &mut self.project {
                    project.close_buffer(&path);
                }
            }
            Command::Forget { path } => {
                self.sent.remove(&canonical_or_same(&path));
            }
            Command::FilesChanged => self.rescan = true,
            Command::Shutdown => {}
        }
    }

    fn answer(&self, query: Query) {
        match query {
            Query::ProjectFiles(reply) => {
                let files = match &self.project {
                    Some((project, _)) => {
                        let bundle = project.graph();
                        let graph = bundle.module_graph();
                        (0..graph.len() as u32)
                            .map(FileId::new)
                            .filter(|id| {
                                graph.is_checkable(*id) && !graph.is_dependency_declaration(*id)
                            })
                            .filter_map(|id| graph.files().path(id))
                            .filter(|path| {
                                !path
                                    .components()
                                    .any(|part| part.as_os_str() == "node_modules")
                            })
                            .map(Path::to_path_buf)
                            .collect()
                    }
                    None => Vec::new(),
                };
                let _ = reply.send(files);
            }
            Query::TypeAt {
                path,
                offset,
                version,
                reply,
            } => {
                let held = self
                    .open
                    .get(&canonical_or_same(&path))
                    .map(|(held, _)| *held);
                // The offset is into one version of the text; a type from another is no answer.
                let current = match (version, held) {
                    (Some(asked), Some(held)) => asked == held,
                    _ => true,
                };
                let found = self
                    .project
                    .as_ref()
                    .filter(|_| current)
                    .and_then(|(project, _)| {
                        // A panic while probing is the project's trouble, not the thread's.
                        catch_unwind(AssertUnwindSafe(|| {
                            project.type_at(&canonical_or_same(&path), offset)
                        }))
                        .ok()
                        .flatten()
                    });
                let _ = reply.send(found);
            }
        }
    }

    fn emit(&self, event: Event) {
        // A closed receiver means the server is gone; the loop notices on its next turn.
        let _ = self.events.send(event);
    }

    fn check(&mut self, token: &CancelToken) {
        if self.project.is_none() {
            match self.open_project() {
                Ok(Some(opened)) => self.project = Some(opened),
                Ok(None) => return,
                Err(message) => {
                    self.emit(Event::Error(message));
                    return;
                }
            }
        }
        let Some((project, _)) = &mut self.project else {
            return;
        };
        if std::mem::take(&mut self.rescan) && project.entries_changed() {
            if let Err(message) = project.rebuild() {
                self.emit(Event::Error(message));
                return;
            }
        }
        let outcome = catch_unwind(AssertUnwindSafe(|| project.check_cancellable(token)));
        let report = match outcome {
            Ok(Ok(report)) => report,
            Ok(Err(message)) => {
                // A newer command overtook this check; its own check will report.
                if !token.is_cancelled() {
                    self.emit(Event::Error(message));
                }
                return;
            }
            Err(_) => {
                // The state of the project after a panic is not to be trusted.
                self.project = None;
                self.emit(Event::Error(
                    "tsox failed while checking; the project will be loaded again".to_owned(),
                ));
                return;
            }
        };
        if token.is_cancelled() {
            return;
        }
        let stats = project.stats();
        let bundle = project.graph();
        let graph = bundle.module_graph();

        let mut by_path: HashMap<PathBuf, Vec<LspDiagnostic>> = HashMap::new();
        for file in &report.files {
            let Some(path) = graph.files().path(file.file_id) else {
                continue;
            };
            let Some((_, text)) = self.open.get(path) else {
                continue;
            };
            let diagnostics = match &file.outcome {
                FileOutcome::Checked(found) => convert::file_to_lsp(found, text),
                FileOutcome::ReadFailed(message) => vec![failure(message)],
                FileOutcome::CheckFailed(error) => vec![failure(&error.to_string())],
            };
            by_path.insert(path.to_path_buf(), diagnostics);
        }

        let mut files = Vec::new();
        for (path, (version, _)) in &self.open {
            // An open document the project does not contain has nothing to report.
            let now = by_path.remove(path).unwrap_or_default();
            let before = self.sent.get(path);
            let unchanged = match before {
                Some(before) => *before == now,
                None => now.is_empty(),
            };
            if unchanged {
                continue;
            }
            self.sent.insert(path.clone(), now.clone());
            files.push(FileUpdate {
                path: path.clone(),
                version: *version,
                diagnostics: now,
            });
        }
        self.emit(Event::Diagnostics { files, stats });
    }

    /// Opens the project: the workspace's `tsconfig.json`, else the nearest one above an open
    /// document, else the open documents themselves. `Ok(None)` when there is nothing to
    /// open yet (no document is open, or none is a file on disk).
    fn open_project(&self) -> Result<Option<(IncrementalProject, Mode)>, String> {
        let config = self
            .root
            .as_deref()
            .map(|root| root.join("tsconfig.json"))
            .filter(|candidate| candidate.is_file())
            .or_else(|| {
                self.open
                    .keys()
                    .find_map(|path| path.parent().and_then(find_tsconfig))
            });
        let (mut project, mode) = match config {
            Some(config) => (IncrementalProject::open(&config)?, Mode::Config),
            None => {
                let entries: BTreeSet<PathBuf> = self
                    .open
                    .keys()
                    .filter(|path| path.is_file())
                    .cloned()
                    .collect();
                if entries.is_empty() {
                    return Ok(None);
                }
                let project = IncrementalProject::open_entries(entries.iter().cloned().collect())?;
                (project, Mode::Entries(entries))
            }
        };
        for (path, (_, text)) in &self.open {
            project.set_buffer(path, text);
        }
        Ok(Some((project, mode)))
    }
}

/// The nearest `tsconfig.json` in `dir` or above it.
pub fn find_tsconfig(dir: &Path) -> Option<PathBuf> {
    dir.ancestors()
        .map(|dir| dir.join("tsconfig.json"))
        .find(|candidate| candidate.is_file())
}

/// A failure that belongs to a whole file, as a diagnostic at its start.
fn failure(message: &str) -> LspDiagnostic {
    LspDiagnostic {
        range: Range {
            start: Position::new(0, 0),
            end: Position::new(0, 0),
        },
        severity: Some(DiagnosticSeverity::ERROR),
        source: Some(SOURCE.to_owned()),
        message: message.to_owned(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_nearest_tsconfig_wins() {
        let dir = tempfile::tempdir().unwrap();
        let inner = dir.path().join("pkg/src");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::write(dir.path().join("tsconfig.json"), "{}").unwrap();
        assert_eq!(
            find_tsconfig(&inner),
            Some(dir.path().join("tsconfig.json"))
        );
        std::fs::write(dir.path().join("pkg/tsconfig.json"), "{}").unwrap();
        assert_eq!(
            find_tsconfig(&inner),
            Some(dir.path().join("pkg/tsconfig.json"))
        );
    }

    #[test]
    fn no_tsconfig_finds_none() {
        let dir = tempfile::tempdir().unwrap();
        // The temp folder's ancestors could hold one on an odd machine; the test is about
        // a folder that has none of its own.
        let found = find_tsconfig(dir.path());
        assert!(found.is_none_or(|path| !path.starts_with(dir.path())));
    }

    fn shared() -> Shared {
        Shared {
            state: Mutex::new(State {
                commands: VecDeque::new(),
                queries: VecDeque::new(),
                token: CancelToken::new(),
                closed: false,
            }),
            ready: Condvar::new(),
        }
    }

    #[test]
    fn a_batch_is_taken_whole_and_the_token_is_new() {
        let shared = shared();
        let old = shared.lock().token.clone();
        {
            let mut state = shared.lock();
            state.token.cancel();
            state.commands.push_back(Command::FilesChanged);
            state.commands.push_back(Command::FilesChanged);
        }
        assert!(old.is_cancelled());
        let (batch, queries, token) = shared.take_batch();
        assert_eq!(batch.len(), 2);
        assert!(queries.is_empty());
        assert!(!token.is_cancelled(), "the new check starts uncancelled");
        // A command sent now overtakes the check that is about to start.
        shared.lock().token.cancel();
        assert!(token.is_cancelled());
    }

    #[test]
    fn a_query_alone_does_not_replace_the_token() {
        let shared = shared();
        let before = shared.lock().token.clone();
        let (reply, _answer) = std::sync::mpsc::channel();
        shared.lock().queries.push_back(Query::ProjectFiles(reply));
        let (batch, queries, token) = shared.take_batch();
        assert!(batch.is_empty());
        assert_eq!(queries.len(), 1);
        // The same token: a check that is running keeps being the one a command cancels.
        before.cancel();
        assert!(token.is_cancelled());
    }

    #[test]
    fn closing_drops_waiting_queries_and_refuses_new_ones() {
        let shared = Arc::new(shared());
        let (reply, answer) = std::sync::mpsc::channel::<Vec<PathBuf>>();
        shared.lock().queries.push_back(Query::ProjectFiles(reply));
        shared.close();
        assert!(answer.recv().is_err(), "the waiting caller is released");
        let queries = Queries {
            shared: Arc::clone(&shared),
        };
        assert_eq!(queries.project_files(), None);
    }

    #[test]
    fn requeued_queries_go_to_the_front_in_order() {
        let shared = shared();
        let (first, _a) = std::sync::mpsc::channel();
        let (second, _b) = std::sync::mpsc::channel();
        let (later, _c) = std::sync::mpsc::channel();
        shared.lock().queries.push_back(Query::ProjectFiles(later));
        shared.requeue(vec![
            Query::ProjectFiles(first),
            Query::ProjectFiles(second),
        ]);
        assert_eq!(shared.lock().queries.len(), 3);
        // The last one in is the one that was asked last: it is still at the back.
        let state = shared.lock();
        assert!(matches!(state.queries[2], Query::ProjectFiles(_)));
    }
}
