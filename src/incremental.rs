use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use salsa::{Database as SalsaDatabase, Durability, Setter};

use crate::types::FileId;

// One project file as the query layer sees it. The file id is a plain u32 and not a
// FileId, so the checker's own ids stay out of Salsa's handle types until a query needs
// them. `file_id` and `path` never change after the input is created; `text` and `hash`
// change only through Database::upsert_source, which keeps the point where a new
// revision starts in one place. The text is an Arc<str> so a worker thread can hold it
// without copying the file.
#[salsa::input(debug)]
pub struct SourceFile {
    #[returns(copy)]
    pub file_id: u32,
    #[returns(deref)]
    pub path: PathBuf,
    #[returns(ref)]
    pub text: Arc<str>,
    #[returns(copy)]
    pub hash: u64,
}

// The first query, here to give the layer something to memoize and to test reuse
// against. Real queries (module header, resolved imports, per-unit checking) come with
// the stages that need them.
#[salsa::tracked(returns(copy))]
pub fn source_len(db: &dyn IncrementalDb, file: SourceFile) -> u32 {
    u32::try_from(file.text(db).len()).unwrap_or(u32::MAX)
}

// Who owns a file decides how long Salsa may trust what it derived from it. Project
// files change on every save. A file under node_modules almost never does, and giving it
// high durability lets a query that reads only such files skip re-verification while
// project files are being edited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    Project,
    Dependency,
}

impl SourceKind {
    fn durability(self) -> Durability {
        match self {
            Self::Project => Durability::LOW,
            Self::Dependency => Durability::HIGH,
        }
    }
}

#[salsa::db]
pub trait IncrementalDb: SalsaDatabase {
    /// Records that a query body ran, so a test can say which queries executed.
    fn note(&self, query: &str);

    /// The token of the check in progress, if the caller gave one. Read by a query that does
    /// long work, so the work can stop; it is not a tracked input.
    fn cancel_token(&self) -> Option<crate::CancelToken>;

    /// The project data of the epoch in force, read without a dependency. A query that reads
    /// it must also read the tracked projection of that data which says when it matters.
    fn current_project(&self) -> Option<Arc<dyn std::any::Any + Send + Sync>>;

    /// What the ambient providers declared in the check in progress; the key of every unit
    /// query carries a digest of it, so a unit never reads a set it was not keyed by.
    fn current_providers(&self) -> Option<Arc<dyn std::any::Any + Send + Sync>>;
}

#[cfg(feature = "module-resolution")]
mod project;
pub use project::{CheckStats, IncrementalProject, ModuleHeader, SecondPass};

// The database the project drives. `files` maps the checker's FileId to its input, so a
// file is updated in place and never gets a second input for the same id.
#[salsa::db]
#[derive(Clone, Default)]
pub struct Database {
    storage: salsa::Storage<Self>,
    // Behind an Arc: a worker handle is a clone of the database, and a map per handle made
    // every handle cost as much as the project is large. Written only through
    // `Arc::make_mut`, which copies when a handle still holds the map.
    files: Arc<BTreeMap<FileId, SourceFile>>,
    // The same inputs by path, for the project layer, whose file ids change with the graph.
    by_path: Arc<BTreeMap<PathBuf, SourceFile>>,
    // The queries that ran, in order; shared by every handle of the database.
    log: Arc<std::sync::Mutex<Vec<String>>>,
    // The caller's cancel token for the check in progress; shared by every handle.
    cancel: Arc<std::sync::Mutex<Option<crate::CancelToken>>>,
    // The project data of the epoch in force, for queries that read it untracked.
    current: Arc<std::sync::RwLock<Option<Arc<dyn std::any::Any + Send + Sync>>>>,
    providers: Arc<std::sync::RwLock<Option<Arc<dyn std::any::Any + Send + Sync>>>>,
}

#[salsa::db]
impl SalsaDatabase for Database {}

#[salsa::db]
impl IncrementalDb for Database {
    fn note(&self, query: &str) {
        self.log
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(query.to_owned());
    }

    fn cancel_token(&self) -> Option<crate::CancelToken> {
        self.cancel
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn current_project(&self) -> Option<Arc<dyn std::any::Any + Send + Sync>> {
        self.current
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn current_providers(&self) -> Option<Arc<dyn std::any::Any + Send + Sync>> {
        self.providers
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

impl Database {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the project data of the epoch in force; see `IncrementalDb::current_project`.
    pub fn set_current_project(&self, data: Arc<dyn std::any::Any + Send + Sync>) {
        *self.current.write().unwrap_or_else(|e| e.into_inner()) = Some(data);
    }

    /// Sets what the ambient providers declared; see `IncrementalDb::current_providers`.
    pub fn set_providers(&self, data: Arc<dyn std::any::Any + Send + Sync>) {
        *self.providers.write().unwrap_or_else(|e| e.into_inner()) = Some(data);
    }

    /// Sets (or clears) the token the queries of the next check watch.
    pub fn set_cancel_token(&self, token: Option<crate::CancelToken>) {
        *self.cancel.lock().unwrap_or_else(|e| e.into_inner()) = token;
    }

    /// The queries that ran since the last call.
    pub fn take_log(&self) -> Vec<String> {
        std::mem::take(&mut *self.log.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Like `upsert_source`, keyed by path. Equal bytes (by hash) change nothing.
    #[cfg(feature = "module-resolution")]
    pub fn upsert_path(
        &mut self,
        path: &std::path::Path,
        text: Arc<str>,
        hash: u64,
        kind: SourceKind,
    ) -> SourceFile {
        if let Some(existing) = self.by_path.get(path).copied() {
            if existing.hash(self) != hash {
                existing.set_text(self).to(text);
                existing.set_hash(self).to(hash);
            }
            return existing;
        }
        let durability = kind.durability();
        let input = SourceFile::builder(self.by_path.len() as u32, path.to_path_buf(), text, hash)
            .file_id_durability(Durability::HIGH)
            .path_durability(Durability::HIGH)
            .text_durability(durability)
            .hash_durability(durability)
            .new(self);
        Arc::make_mut(&mut self.by_path).insert(path.to_path_buf(), input);
        input
    }

    pub fn source(&self, file: FileId) -> Option<SourceFile> {
        self.files.get(&file).copied()
    }

    /// Creates the input for `file`, or brings it up to date. A hash equal to the stored
    /// one changes nothing, so a save without edits starts no revision and every query
    /// that read the file keeps its result. Otherwise the input is rewritten in place.
    pub fn upsert_source(
        &mut self,
        file: FileId,
        path: PathBuf,
        text: Arc<str>,
        hash: u64,
        kind: SourceKind,
    ) -> SourceFile {
        if let Some(existing) = self.source(file) {
            if existing.hash(self) != hash {
                existing.set_text(self).to(text);
                existing.set_hash(self).to(hash);
            }
            return existing;
        }

        let durability = kind.durability();
        let input = SourceFile::builder(file.index(), path, text, hash)
            .file_id_durability(Durability::HIGH)
            .path_durability(Durability::HIGH)
            .text_durability(durability)
            .hash_durability(durability)
            .new(self);
        Arc::make_mut(&mut self.files).insert(file, input);
        input
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::{Database, SourceKind, source_len};
    use crate::types::FileId;

    // A database that counts query executions, so a test can tell a result that was
    // recomputed from one that was reused.
    fn counting_database() -> (Database, Arc<AtomicUsize>) {
        let executions = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&executions);
        let database = Database {
            storage: salsa::Storage::new(Some(Box::new(move |event| {
                if let salsa::EventKind::WillExecute { .. } = event.kind {
                    counter.fetch_add(1, Ordering::SeqCst);
                }
            }))),
            files: Arc::default(),
            by_path: Arc::default(),
            log: Arc::default(),
            cancel: Arc::default(),
            current: Arc::default(),
            providers: Arc::default(),
        };
        (database, executions)
    }

    fn id(index: u32) -> FileId {
        FileId::new(index)
    }

    #[test]
    fn a_save_without_edits_does_not_rerun_the_query() {
        let (mut db, executions) = counting_database();
        let file = db.upsert_source(
            id(7),
            PathBuf::from("src/a.ts"),
            Arc::from("const a = 1;"),
            11,
            SourceKind::Project,
        );
        assert_eq!(source_len(&db, file), 12);
        assert_eq!(executions.load(Ordering::SeqCst), 1);

        let again = db.upsert_source(
            id(7),
            PathBuf::from("src/a.ts"),
            Arc::from("const a = 1;"),
            11,
            SourceKind::Project,
        );
        assert_eq!(again, file);
        assert_eq!(source_len(&db, again), 12);
        assert_eq!(executions.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_changed_hash_updates_the_input_in_place_and_reruns_the_query() {
        let (mut db, executions) = counting_database();
        let file = db.upsert_source(
            id(1),
            PathBuf::from("a.ts"),
            Arc::from("a"),
            10,
            SourceKind::Project,
        );
        assert_eq!(source_len(&db, file), 1);

        let updated = db.upsert_source(
            id(1),
            PathBuf::from("a.ts"),
            Arc::from("ab"),
            20,
            SourceKind::Project,
        );
        assert_eq!(updated, file);
        assert_eq!(source_len(&db, updated), 2);
        assert_eq!(executions.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_file_is_found_by_its_file_id() {
        let mut db = Database::new();
        assert!(db.source(id(3)).is_none());
        let file = db.upsert_source(
            id(3),
            PathBuf::from("node_modules/p/index.d.ts"),
            Arc::from("export {};"),
            5,
            SourceKind::Dependency,
        );
        assert_eq!(db.source(id(3)), Some(file));
        assert_eq!(file.file_id(&db), 3);
        assert_eq!(file.path(&db), Path::new("node_modules/p/index.d.ts"));
    }

    // The behavior E7 depends on, measured on the pinned Salsa (0.28.5) before any real query
    // is built on it. Each test names what the design may rely on. The queries below count
    // their own runs in statics, so the tests take one lock and run one at a time.
    mod spike {
        use std::path::PathBuf;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::{Arc, Mutex};
        use std::time::{Duration, Instant};

        use salsa::{Cancelled, Database as _};

        use super::super::{Database, IncrementalDb, SourceFile, SourceKind};
        use crate::types::FileId;

        static SERIAL: Mutex<()> = Mutex::new(());
        static TOKEN_RUNS: AtomicUsize = AtomicUsize::new(0);
        static DOUBLED_RUNS: AtomicUsize = AtomicUsize::new(0);

        // Stands for a module header: what other files read from this one.
        #[salsa::tracked(returns(copy))]
        fn tokens(db: &dyn IncrementalDb, file: SourceFile) -> u32 {
            TOKEN_RUNS.fetch_add(1, Ordering::SeqCst);
            u32::try_from(file.text(db).split_whitespace().count()).unwrap()
        }

        // Stands for a dependent unit: reads only the header, never the text.
        #[salsa::tracked(returns(copy))]
        fn doubled(db: &dyn IncrementalDb, file: SourceFile) -> u32 {
            DOUBLED_RUNS.fetch_add(1, Ordering::SeqCst);
            tokens(db, file) * 2
        }

        // A long check: polls for cancellation the way a worker must between statements.
        #[salsa::tracked(returns(copy))]
        fn long_check(db: &dyn IncrementalDb, file: SourceFile) -> u32 {
            let _ = file.text(db);
            loop {
                db.unwind_if_revision_cancelled();
                std::thread::sleep(Duration::from_millis(1));
            }
        }

        fn file(db: &mut Database, index: u32, text: &str, hash: u64) -> SourceFile {
            db.upsert_source(
                FileId::new(index),
                PathBuf::from(format!("f{index}.ts")),
                Arc::from(text),
                hash,
                SourceKind::Project,
            )
        }

        fn reset() {
            TOKEN_RUNS.store(0, Ordering::SeqCst);
            DOUBLED_RUNS.store(0, Ordering::SeqCst);
        }

        #[test]
        fn early_cutoff_stops_at_a_query_whose_result_did_not_change() {
            let _lock = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
            reset();
            let mut db = Database::new();
            let a = file(&mut db, 1, "const a = 1;", 1);
            assert_eq!(doubled(&db, a), 8);
            // Whitespace only: the text changed, the token count did not.
            let a = file(&mut db, 1, "const   a  =  1;\n", 2);
            assert_eq!(doubled(&db, a), 8);
            assert_eq!(TOKEN_RUNS.load(Ordering::SeqCst), 2, "the header reran");
            assert_eq!(
                DOUBLED_RUNS.load(Ordering::SeqCst),
                1,
                "the dependent did not"
            );
            // A real change reaches the dependent.
            let a = file(&mut db, 1, "const a = 1; const b = 2;", 3);
            assert_eq!(doubled(&db, a), 16);
            assert_eq!(DOUBLED_RUNS.load(Ordering::SeqCst), 2);
        }

        #[test]
        fn editing_one_file_leaves_the_queries_of_another_alone() {
            let _lock = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
            reset();
            let mut db = Database::new();
            let a = file(&mut db, 1, "a b", 1);
            let b = file(&mut db, 2, "c d e", 2);
            assert_eq!((doubled(&db, a), doubled(&db, b)), (4, 6));
            reset();
            let a = file(&mut db, 1, "a b c", 3);
            assert_eq!((doubled(&db, a), doubled(&db, b)), (6, 6));
            assert_eq!(
                TOKEN_RUNS.load(Ordering::SeqCst),
                1,
                "only a's header reran"
            );
            assert_eq!(DOUBLED_RUNS.load(Ordering::SeqCst), 1);
        }

        #[test]
        fn a_thread_handle_is_a_clone_and_one_query_runs_once() {
            let _lock = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
            reset();
            let mut db = Database::new();
            let a = file(&mut db, 1, "a b c d", 1);
            let results: Vec<u32> = (0..8)
                .map(|_| db.clone())
                .map(|handle| std::thread::spawn(move || doubled(&handle, a)))
                .collect::<Vec<_>>()
                .into_iter()
                .map(|thread| thread.join().unwrap())
                .collect();
            assert_eq!(results, vec![8; 8]);
            assert_eq!(
                TOKEN_RUNS.load(Ordering::SeqCst),
                1,
                "computed once across 8 threads"
            );
            assert_eq!(DOUBLED_RUNS.load(Ordering::SeqCst), 1);
        }

        #[test]
        fn a_write_cancels_a_running_query_and_waits_for_it_to_unwind() {
            let _lock = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
            let mut db = Database::new();
            let a = file(&mut db, 1, "a", 1);
            let handle = db.clone();
            let started = Arc::new(AtomicUsize::new(0));
            let flag = Arc::clone(&started);
            let worker = std::thread::spawn(move || {
                flag.store(1, Ordering::SeqCst);
                Cancelled::catch(|| long_check(&handle, a))
            });
            while started.load(Ordering::SeqCst) == 0 {}
            std::thread::sleep(Duration::from_millis(20));
            let begin = Instant::now();
            // The setter cancels the revision and returns once every handle is dropped.
            let _ = file(&mut db, 1, "a b", 2);
            let waited = begin.elapsed();
            let outcome = worker.join().unwrap();
            assert!(
                matches!(outcome, Err(Cancelled::PendingWrite)),
                "{outcome:?}"
            );
            assert!(
                waited < Duration::from_secs(2),
                "the write waited {waited:?}"
            );
        }

        #[test]
        fn trigger_cancellation_stops_a_query_without_a_write() {
            let _lock = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
            let mut db = Database::new();
            let a = file(&mut db, 1, "a", 1);
            let handle = db.clone();
            let token = handle.cancellation_token();
            let worker = std::thread::spawn(move || Cancelled::catch(|| long_check(&handle, a)));
            std::thread::sleep(Duration::from_millis(20));
            token.cancel();
            let outcome = worker.join().unwrap();
            assert!(matches!(outcome, Err(Cancelled::Local)), "{outcome:?}");
        }
    }
}
