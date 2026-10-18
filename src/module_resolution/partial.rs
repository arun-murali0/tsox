use std::collections::{BTreeMap, BTreeSet};

use crate::bridge::IncompleteReason;
use crate::types::FileId;

use super::check::ProjectReport;
use super::graph::ModuleGraph;

// Problem: --partial listed every file whose exports are partial and why, but not how far a
// partial file reaches. One declaration that cannot be exported also makes every file that
// re-exports from it partial, so the list reads as a dozen problems when it is one.
// Picked: the reasons are split into the ones that start a partial list and the two that only
// pass one on (`export *` from a module whose list was read and is partial). A star over a
// module that is missing or was not read is the file's own problem, so it is a root cause.
// Files are counted under the first kind, and each row says how many project files import them
// directly and through other files.
// Cost: a dependent is any project file that imports a partial one, whether or not it reads
// a name that is missing, so the figure is a reach and not a count of wrong answers. A dynamic
// import is not a dependency edge (nothing reads what `import()` exports yet), so a file that
// only reaches a partial one that way is not counted.

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReasonKind {
    /// The collector gave up on something in the file itself.
    Root,
    /// A re-export that can come from a missing module, a name the source does not export,
    /// or a source whose own list is partial. The reason does not say which.
    Mixed,
    /// The file is partial only because a module it re-exports from is.
    Propagation,
}

impl ReasonKind {
    pub fn of(reason: IncompleteReason) -> Self {
        match reason {
            IncompleteReason::CheckerCrashed
            | IncompleteReason::UnsupportedDeclaration
            | IncompleteReason::EmptyBindingPattern
            | IncompleteReason::ShorthandAmbientModule
            | IncompleteReason::AnonymousDefaultWithoutSignature
            | IncompleteReason::NonObjectExportAssignment
            | IncompleteReason::UnknownLocalExport
            | IncompleteReason::DeclarationFileWithoutExportStatement
            | IncompleteReason::UnrepresentableDeclaration
            | IncompleteReason::StarFromUnknownModule
            | IncompleteReason::NamespaceStarFromUnknownModule => Self::Root,
            IncompleteReason::UnresolvedReExport => Self::Mixed,
            IncompleteReason::StarFromPartialModule
            | IncompleteReason::NamespaceStarFromPartialModule => Self::Propagation,
        }
    }

    fn heading(self) -> &'static str {
        match self {
            Self::Root => "root causes",
            Self::Mixed => "root cause or passed on",
            Self::Propagation => "passed on",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartialRow {
    pub reason: IncompleteReason,
    pub kind: ReasonKind,
    /// Files whose list is partial for this reason. A file with two reasons is in two rows.
    pub files: usize,
    /// Project files that import one of those files.
    pub direct_dependents: usize,
    /// Project files that import one of those files, directly or through other files.
    pub reached_dependents: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PartialSummary {
    /// Files with a partial list, each counted once.
    pub partial_files: usize,
    /// Of those, files with at least one reason that is not pure pass-through.
    pub root_files: usize,
    /// Of those, files that are partial only because a module they re-export from is.
    pub passed_on_only: usize,
    /// Root causes first, then mixed, then passed on; most files first inside each.
    pub rows: Vec<PartialRow>,
}

pub fn summarize_partial(graph: &ModuleGraph, report: &ProjectReport) -> PartialSummary {
    let mut summary = PartialSummary::default();
    let mut by_reason: BTreeMap<IncompleteReason, BTreeSet<FileId>> = BTreeMap::new();

    for file in &report.files {
        if file.incomplete_reasons.is_empty() {
            continue;
        }
        summary.partial_files += 1;
        let passed_on_only = file
            .incomplete_reasons
            .iter()
            .all(|reason| ReasonKind::of(*reason) == ReasonKind::Propagation);
        if passed_on_only {
            summary.passed_on_only += 1;
        } else {
            summary.root_files += 1;
        }
        for reason in &file.incomplete_reasons {
            by_reason.entry(*reason).or_default().insert(file.file_id);
        }
    }

    for (reason, files) in by_reason {
        let direct: BTreeSet<FileId> = files
            .iter()
            .flat_map(|file| graph.dependents(*file).iter().copied())
            .filter(|id| graph.is_checkable(*id) && !files.contains(id))
            .collect();
        summary.rows.push(PartialRow {
            reason,
            kind: ReasonKind::of(reason),
            files: files.len(),
            direct_dependents: direct.len(),
            reached_dependents: reached(graph, &files).len(),
        });
    }

    summary.rows.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then(b.files.cmp(&a.files))
            .then(a.reason.cmp(&b.reason))
    });
    summary
}

// Every project file that imports one of `start`, directly or through other files, not
// counting `start` itself. Files in an import cycle are visited once.
fn reached(graph: &ModuleGraph, start: &BTreeSet<FileId>) -> BTreeSet<FileId> {
    let mut seen: BTreeSet<FileId> = start.clone();
    let mut pending: Vec<FileId> = start.iter().copied().collect();
    let mut found = BTreeSet::new();
    while let Some(file) = pending.pop() {
        for dependent in graph.dependents(file) {
            if seen.insert(*dependent) {
                pending.push(*dependent);
                if graph.is_checkable(*dependent) {
                    found.insert(*dependent);
                }
            }
        }
    }
    found
}

impl PartialSummary {
    /// The lines `--partial` prints under the per-file list. Empty when nothing is partial.
    pub fn lines(&self) -> Vec<String> {
        if self.partial_files == 0 {
            return Vec::new();
        }
        let mut lines = vec![format!(
            "partial exports: {} file(s), {} from a root cause or a mixed reason, {} only passed on by `export *`",
            self.partial_files, self.root_files, self.passed_on_only
        )];
        for kind in [ReasonKind::Root, ReasonKind::Mixed, ReasonKind::Propagation] {
            let rows: Vec<&PartialRow> = self.rows.iter().filter(|row| row.kind == kind).collect();
            if rows.is_empty() {
                continue;
            }
            lines.push(format!("  {}:", kind.heading()));
            lines.push("    files  direct  reached  reason".to_string());
            for row in rows {
                lines.push(format!(
                    "    {:>5}  {:>6}  {:>7}  {:?}: {}",
                    row.files,
                    row.direct_dependents,
                    row.reached_dependents,
                    row.reason,
                    row.reason.describe()
                ));
            }
        }
        lines
    }
}
