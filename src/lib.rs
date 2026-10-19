mod arena;
mod bridge;
mod diagnostic_codes;
mod diagnostic_messages;
mod diagnostic_view;
mod diagnostics;
mod error;
mod explain;
mod fxhash;
#[cfg(feature = "incremental")]
mod incremental;
mod keys;
mod lib_global_types;
mod line_index;
#[cfg(feature = "module-resolution")]
mod module_resolution;
mod namespace;
#[allow(dead_code)]
mod portable;
mod project;
mod scc;
#[cfg(feature = "scratchpad")]
mod scratchpad;
mod semantic;
mod source_depth;
#[allow(dead_code)]
mod stable_hash;
#[allow(dead_code)]
mod stable_walk;
mod stack_guard;
mod subtyping;
mod symbol_map;
#[cfg(feature = "module-resolution")]
mod topology;
mod type_annotation;
mod type_display;
mod types;
mod unit;

#[cfg(feature = "wasm")]
mod wasm;
#[cfg(feature = "wasm")]
pub use wasm::TsoxChecker;

pub use diagnostic_codes::DiagnosticCode;
pub use diagnostic_view::{DiagnosticView, Position, Range};
pub use diagnostics::{Diagnostic, Severity};
pub use error::CheckerError;

pub use keys::{
    CycleKey, DeclKey, DeclKind, ENC_VERSION, ModuleKey, NodeHash, SigHash, TypeParamKey,
};
pub use line_index::LineIndex;
pub use portable::{
    DeclProvider, GlobalNamespace, ImportSource, ModuleExports, PNode, PParam, PProp, PTypeParam,
    PortableDecl, PortableType,
};
pub use stable_hash::Prim;
pub use stack_guard::with_deep_stack;
pub use unit::{AmbientExports, CancelToken, UnitContext, UnitError, UnitOutput, Worker};

#[cfg(feature = "incremental")]
pub use incremental::{
    Database as IncrementalDatabase, IncrementalDb, SourceFile, SourceKind, source_len,
};

#[cfg(feature = "scratchpad")]
pub use scratchpad::WorkerScratch;

#[cfg(feature = "module-resolution")]
pub use topology::ModuleTopology;

#[cfg(feature = "module-resolution")]
pub use module_resolution::{
    FileFingerprint, FileOutcome, FileReport, ModuleEdge, ModuleError, ModuleGraph, ModuleRequest,
    ModuleRequestKind, ModuleResolver, ModuleScan, PartialRow, PartialSummary, ProjectReport,
    ReasonKind, check_project, content_hash, scan_module_requests, summarize_partial,
};

// Exposed only so a test can make an import cycle run out of recheck rounds (see
// tests/project_imports.rs). Hidden from generated docs since it is not part of the
// supported public API.
#[cfg(feature = "module-resolution")]
#[doc(hidden)]
pub use module_resolution::check_project_bounded;

pub use arena::TypeArenaStats;
pub use bridge::{CheckMetrics, IncompleteReason};
pub use namespace::NamespaceStats;
pub use project::ProjectFiles;
pub use semantic::queries::QueryStats;
pub use types::FileId;

// Exposed only for the parse and bind only benchmark (see
// benches/checker_benchmark.rs), which needs to measure the declare pass in
// isolation from full type checking. Hidden from generated docs since it is not
// part of the supported public API.
#[doc(hidden)]
pub use bridge::parse_and_bind_only;

#[derive(Default)]
pub struct TypeChecker {}

// The explicit mutable boundary for checking one logical file over and over, as an
// editor or a watch mode does. The reusable state (the type arena and its allocation)
// lives here and not inside TypeChecker, so TypeChecker stays free of interior
// mutability and independent sessions can run side by side, one per file, which is the
// path to checking files in parallel.
pub struct CheckSession {
    file_id: FileId,
    arena: arena::TypeArena,
    last_metrics: Option<CheckMetrics>,
}

impl CheckSession {
    pub fn new(file_id: FileId) -> Self {
        Self {
            file_id,
            arena: arena::TypeArena::new(),
            last_metrics: None,
        }
    }

    pub fn file_id(&self) -> FileId {
        self.file_id
    }

    /// Metrics from the most recent successful check. They live on the session so a
    /// regression is observable without changing CheckResult or instrumenting every
    /// semantic operation.
    pub fn last_metrics(&self) -> Option<&CheckMetrics> {
        self.last_metrics.as_ref()
    }

    #[tracing::instrument(skip(self, source), fields(file_id = self.file_id.index(), file_name = file_name))]
    pub fn check_source(
        &mut self,
        source: &str,
        file_name: &str,
    ) -> Result<CheckResult, CheckerError> {
        let (diagnostics, metrics) =
            bridge::check_program_with_state(source, file_name, self.file_id, &mut self.arena)?;
        self.last_metrics = Some(metrics);
        Ok(CheckResult { diagnostics })
    }
}

#[derive(Debug)]
pub struct CheckResult {
    pub diagnostics: Vec<Diagnostic>,
}

impl TypeChecker {
    pub fn new() -> Self {
        Self::default()
    }

    /// A session for repeated checks of one logical file. `check_source` below is the
    /// one-shot form; a caller that re-checks keeps the session and reuses its arena.
    pub fn session(&self, file_id: FileId) -> CheckSession {
        CheckSession::new(file_id)
    }

    #[tracing::instrument(skip(self, source), fields(file_name = file_name))]
    pub fn check_source(&self, source: &str, file_name: &str) -> Result<CheckResult, CheckerError> {
        let diagnostics = bridge::check_program(source, file_name)?;
        Ok(CheckResult { diagnostics })
    }
}

#[cfg(test)]
mod tests {
    use super::{FileId, TypeChecker};

    #[test]
    fn a_session_rechecks_a_file_without_carrying_old_state() {
        let checker = TypeChecker::new();
        let mut session = checker.session(FileId::new(7));

        let first = session
            .check_source(
                "type Box<T> = { value: T }; const a: Box<number> = { value: 1 };",
                "file.ts",
            )
            .unwrap();
        assert!(first.diagnostics.is_empty(), "first check: {first:?}");

        let second = session
            .check_source("const value: string = 'ok';", "file.ts")
            .unwrap();
        assert!(second.diagnostics.is_empty(), "second check: {second:?}");
        assert_eq!(session.file_id(), FileId::new(7));
    }

    #[test]
    fn a_session_check_matches_a_one_shot_check() {
        let source = "const n: number = 'x'; function f(a: string): number { return a; }";
        let one_shot = TypeChecker::new().check_source(source, "m.ts").unwrap();
        let mut session = TypeChecker::new().session(FileId::ROOT);
        // Run twice: the second run starts from a reused, cleared arena.
        session.check_source(source, "m.ts").unwrap();
        let reused = session.check_source(source, "m.ts").unwrap();

        let codes = |r: &super::CheckResult| {
            r.diagnostics
                .iter()
                .map(|d| (d.code, d.start))
                .collect::<Vec<_>>()
        };
        assert_eq!(codes(&one_shot), codes(&reused));
        assert!(!one_shot.diagnostics.is_empty());
    }

    #[test]
    fn session_metrics_describe_the_last_check() {
        let mut session = TypeChecker::new().session(FileId::ROOT);
        assert!(session.last_metrics().is_none());
        session
            .check_source("const a: number = 1;", "m.ts")
            .unwrap();
        let metrics = session.last_metrics().expect("metrics after a check");
        assert!(
            metrics.arena.type_count >= 10,
            "at least the ten primitives"
        );
    }
}
