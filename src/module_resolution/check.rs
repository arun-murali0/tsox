use std::borrow::Cow;
use std::fs;
use std::sync::{Arc, OnceLock, RwLock};

use rayon::prelude::*;

use crate::bridge::IncompleteReason;
use crate::diagnostics::Diagnostic;
use crate::error::CheckerError;
use crate::fxhash::FxHashMap;
use crate::keys::{DeclKey, DeclKind};
use crate::portable::{DeclProvider, GlobalNamespace, ImportSource, ModuleExports, PortableDecl};
use crate::types::FileId;
use crate::unit::{CancelToken, UnitContext, UnitError, Worker};

use super::graph::ModuleGraph;
use super::schedule::{UnitOutcome, run_units, unit_dependencies};

/// Text for a file that replaces what the graph kept (the query layer holds the text it checks).
pub(crate) type TextFn = dyn Fn(FileId) -> Option<Arc<str>> + Sync;

/// A source of finished units for the first pass: a unit it answers is not checked again. The
/// query layer implements it (see `incremental`); `None` means "check it as usual".
pub(crate) trait UnitMemo: Sync {
    fn unit(&self, unit: usize) -> Option<UnitResult>;

    /// The result of one ambient provider, which runs before the first pass. A memo that keeps
    /// them answers here and is told what they declared by `providers_ready`, so it can answer
    /// the units of the pass; `None` leaves the providers to the driver (and the units then are
    /// not memoized). Called from the worker pool.
    fn provider(&self, _id: FileId) -> Option<Checked> {
        None
    }

    /// Called once the providers have all answered, in file order, before the first pass.
    fn providers_ready(&self, _units: &[(FileId, UnitExports)]) {}

    /// A digest of everything the first pass was answered from, or `None` when some unit was
    /// not answered by the memo (the second pass is then never kept or reused). Two equal
    /// digests mean the first pass handed the second the same thing.
    fn first_pass_fingerprint(&self) -> Option<u128> {
        None
    }

    /// The second pass's result kept under `fingerprint`, if that is the one held.
    fn second_pass(&self, _fingerprint: u128) -> Option<UnitResult> {
        None
    }

    /// Keeps the second pass's result under `fingerprint`, replacing any other.
    fn keep_second_pass(&self, _fingerprint: u128, _result: &UnitResult) {}
}

// What the second pass reads, besides the first pass's results: the text of every file it
// checks again, which files those are, and the graph's epoch (through the first pass's
// fingerprint). A file whose first-pass output did not change can still come out differently
// in the second pass, because the second pass sees names the first could not: a body edit that
// changes a call to a global declaration changes nothing the first pass reports.
fn second_pass_fingerprint(
    memo: &dyn UnitMemo,
    graph: &ModuleGraph,
    texts: &TextFn,
    affected: Option<&std::collections::HashSet<FileId>>,
    scripts: &[FileId],
    wildcard_declared: bool,
) -> Option<u128> {
    let first = memo.first_pass_fingerprint()?;
    let mut ids: Vec<FileId> = match affected {
        Some(set) => set.iter().copied().chain(scripts.iter().copied()).collect(),
        None => (0..graph.len()).map(|i| FileId::new(i as u32)).collect(),
    };
    ids.sort();
    ids.dedup();
    let mut hasher = xxhash_rust::xxh3::Xxh3::new();
    hasher.update(&first.to_le_bytes());
    hasher.update(&[u8::from(affected.is_some()), u8::from(wildcard_declared)]);
    hasher.update(&(ids.len() as u64).to_le_bytes());
    for id in ids {
        hasher.update(&id.index().to_le_bytes());
        match texts(id) {
            Some(text) => {
                hasher.update(&[1]);
                hasher.update(&(text.len() as u64).to_le_bytes());
                hasher.update(text.as_bytes());
            }
            None => {
                hasher.update(&[0]);
            }
        }
    }
    Some(hasher.digest128())
}

// A result worth keeping: no file crashed the checker and none was stopped by the token.
fn second_pass_is_keepable(result: &UnitResult) -> bool {
    result.files.iter().all(|(report, _)| {
        !report
            .incomplete_reasons
            .contains(&IncompleteReason::CheckerCrashed)
            && !matches!(&report.outcome, FileOutcome::ReadFailed(text) if text == CANCELLED)
    })
}

/// What the callers of `run_project` may hand over besides the graph.
#[derive(Default)]
pub(crate) struct RunOptions<'a> {
    pub texts: Option<&'a TextFn>,
    pub memo: Option<&'a dyn UnitMemo>,
}

// What happened to one file. A file that could not be read or parsed is reported as
// such and is never left out, because a report that silently omits files reads as "all
// clear" for exactly the files nobody looked at.
#[derive(Clone, Debug)]
pub enum FileOutcome {
    Checked(Vec<Diagnostic>),
    ReadFailed(String),
    CheckFailed(CheckerError),
}

#[derive(Clone, Debug)]
pub struct FileReport {
    pub file_id: FileId,
    pub outcome: FileOutcome,
    /// The declarations of other files this file's check read, sorted and without
    /// duplicates. A key carries its module, so this is both the file-level dependency
    /// and the finer one: a change that leaves all of these signatures alone cannot
    /// change this file's diagnostics (LLD 6.2).
    pub reads: Vec<DeclKey>,
    /// Why this file's exports are partial, sorted and without duplicates; empty when they
    /// are complete. A file that imports a partial one stays quiet about a missing name.
    pub incomplete_reasons: Vec<IncompleteReason>,
}

#[derive(Debug, Default)]
pub struct ProjectReport {
    pub files: Vec<FileReport>,
    /// Members of import cycles whose exports were still changing when the recheck bound
    /// ran out, sorted. Such a file's exports are handed on as incomplete, so a file that
    /// imports it loses an error and never invents one, but the file's own diagnostics
    /// were computed from a snapshot that had not settled. Empty when every cycle settled.
    pub unsettled: Vec<FileId>,
    /// True when the cancel token stopped the run. The files that did not run are in `files`
    /// as read failures that say so, so a cancelled run does not read as a clean one.
    pub cancelled: bool,
}

impl ProjectReport {
    pub fn diagnostic_count(&self) -> usize {
        self.files
            .iter()
            .map(|report| match &report.outcome {
                FileOutcome::Checked(diagnostics) => diagnostics.len(),
                FileOutcome::ReadFailed(_) | FileOutcome::CheckFailed(_) => 0,
            })
            .sum()
    }

    /// Files that were not checked at all, as opposed to checked and found wrong.
    pub fn failure_count(&self) -> usize {
        self.files
            .iter()
            .filter(|report| !matches!(report.outcome, FileOutcome::Checked(_)))
            .count()
    }
}

// Problem: the project driver checked one graph layer at a time and waited at the end of
// each, so a file that was ready the moment its imports finished still waited for the
// slowest file of its layer. It also checked every file twice, because the test for "does
// a second pass have anything new to see" asked whether the table of UMD aliases was empty,
// and that table got an entry for every file, empty or not.
// Picked: the units are the components of the graph (a cycle is one unit, as in E3), run on
// the ready queue of `schedule.rs`. A unit reads only what the units it waits for published
// (their export names and declarations, by file), plus a `Globals` value that is frozen for
// the whole pass: the ambient modules, the augmentations and the UMD namespaces. Nothing a
// unit reads depends on which unit finished first, so the report is the same on any number
// of threads. A second pass runs only when the first produced something the first pass
// could not have seen (a UMD alias, an augmentation, an ambient module in a file that is not
// a provider), and then it runs against `Globals` built from everything the first produced.
// Cost: two reads of the same file's declarations can no longer be told apart by timing, so
// a unit must never look at a file it does not wait for. The tables are keyed by file and
// only closed-over files are asked for, which holds by how keys are made, not by a check.

// The text of a report for a file that never ran because the token was cancelled.
const CANCELLED: &str = "the check was cancelled";

/// The checker's own view of a project while it runs: what every finished file exported,
/// and what is true of the whole project in this pass.
struct Project<'a> {
    graph: &'a ModuleGraph,
    tables: RwLock<Tables>,
    globals: Globals,
    cancel: Option<&'a CancelToken>,
    // Whether a script file builds declarations for its global names, or only lists them.
    // Off in the passes; on for the providers and for the run that rebuilds the scripts
    // another file uses (see `run_project`).
    collect_global_decls: bool,
    // Text that replaces the graph's own, when the caller holds the current text itself.
    texts: Option<&'a TextFn>,
    // Check the members of a unit one after another on this thread. The query layer needs
    // it: a query's reads are recorded only on the thread that runs it.
    sequential: bool,
}

impl Project<'_> {
    fn publish(&self, id: FileId, unit: UnitExports) {
        self.tables
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .add(id, unit);
    }

    // Like `publish`, for exports that stay owned by the caller: only what the tables keep is
    // copied (the names and the declaration handles), not the rest of the unit's exports.
    #[cfg(feature = "incremental")]
    fn publish_ref(&self, id: FileId, unit: &UnitExports) {
        self.tables
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .add_ref(id, unit);
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, Tables> {
        self.tables.read().unwrap_or_else(|e| e.into_inner())
    }
}

// What each finished file exports: names, whether the list is whole, and declarations by
// key. Keys carry their module, so one map holds every module's. Only looked up, never
// walked, so hash order cannot reach a result (LLD 8.2).
#[derive(Default)]
struct Tables {
    names: FxHashMap<FileId, Arc<Vec<(String, DeclKey)>>>,
    complete: FxHashMap<FileId, bool>,
    decls: FxHashMap<DeclKey, Arc<PortableDecl>>,
}

impl Tables {
    #[cfg(feature = "incremental")]
    fn add_ref(&mut self, id: FileId, unit: &UnitExports) {
        self.names.insert(id, Arc::clone(&unit.names));
        self.complete.insert(id, unit.complete);
        for decl in &unit.decls {
            self.decls.insert(decl.key, Arc::clone(decl));
        }
    }

    fn add(&mut self, id: FileId, unit: UnitExports) {
        self.names.insert(id, unit.names);
        self.complete.insert(id, unit.complete);
        for decl in unit.decls {
            self.decls.insert(decl.key, decl);
        }
    }
}

// What is true of the project as a whole, fixed before a pass starts and never changed
// during it, so no unit sees a half-built version.
#[derive(Default)]
struct Globals {
    // The modules some file declared with `declare module "x"`, by specifier: their export
    // names, and whether those are the whole list. A specifier several blocks declare is
    // their union.
    ambient: FxHashMap<String, (Vec<(String, DeclKey)>, bool)>,
    // Module augmentation declarations are merged with a resolved module's export list;
    // unlike ambient modules, they must not make an otherwise unresolved specifier exist.
    augmentations: FxHashMap<FileId, (Vec<(String, DeclKey)>, bool)>,
    // The declarations that came with the ambient modules and the augmentations.
    decls: FxHashMap<DeclKey, Arc<PortableDecl>>,
    // The UMD aliases (`export as namespace`), with the members of the file that exports them.
    global_namespaces: Vec<GlobalNamespace>,
    // The names the project declares globally (top-level declarations of script files and
    // members of `declare global` blocks), each with every declaration of it. The files are
    // taken in path order, so the order of a name's declarations does not depend on the
    // order the files were checked or listed in.
    global_names: FxHashMap<String, Vec<DeclKey>>,
    // Names the project declares globally whose declaration could not be carried over. A use
    // of one is not a missing name, but nothing is known about its type.
    unmodelled_globals: std::collections::HashSet<String>,
    // Whether a package specifier no file answers is certainly a missing module (see
    // `bare_specifiers_are_reportable`).
    bare_reportable: bool,
}

impl Globals {
    // Built from the outputs of the given files, which are in file order, so the result
    // does not depend on the order they were produced in.
    fn build(graph: &ModuleGraph, units: &[(FileId, &UnitExports)]) -> Self {
        let mut globals = Globals::default();
        for (id, unit) in units {
            for module in &unit.ambient {
                for decl in &module.exports {
                    globals.decls.insert(decl.key, Arc::new(decl.clone()));
                }
                let entry = globals
                    .ambient
                    .entry(module.specifier.clone())
                    .or_insert_with(|| (Vec::new(), true));
                entry.0.extend(module.export_names.iter().cloned());
                entry.0.sort();
                entry.0.dedup();
                entry.1 &= module.complete;
            }
            for module in &unit.augmentations {
                // Augmentations attach to the resolved module, not to the text of the
                // specifier. Different consumers may name the same file as `./x`, `../p/x`,
                // or through a package export. The graph's edge is the authoritative identity.
                let target = graph
                    .edges(*id)
                    .iter()
                    .find(|edge| {
                        edge.kind == super::discovery::ModuleRequestKind::ModuleAugmentation
                            && edge.specifier == module.specifier
                    })
                    .and_then(|edge| edge.target);
                for decl in &module.exports {
                    globals.decls.insert(decl.key, Arc::new(decl.clone()));
                }
                let Some(target) = target else {
                    continue;
                };
                let entry = globals
                    .augmentations
                    .entry(target)
                    .or_insert_with(|| (Vec::new(), true));
                entry.0.extend(module.export_names.iter().cloned());
                entry.0.sort();
                entry.0.dedup();
                entry.1 &= module.complete;
            }
        }
        globals.global_namespaces = global_namespaces(units);
        // Project globals. Merged in path order (ties by id), so a name declared in several
        // files lists its declarations the same way however the project was assembled.
        let mut by_path: Vec<(Option<&std::path::Path>, FileId, &UnitExports)> = units
            .iter()
            .filter(|(_, unit)| !unit.globals.is_empty())
            .map(|(id, unit)| (graph.files().path(*id), *id, *unit))
            .collect();
        by_path.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
        for (_, _, unit) in &by_path {
            for block in &unit.globals {
                for decl in &block.exports {
                    globals.decls.insert(decl.key, Arc::new(decl.clone()));
                }
                for (name, key) in &block.export_names {
                    let keys = globals.global_names.entry(name.clone()).or_default();
                    if !keys.contains(key) {
                        keys.push(*key);
                    }
                }
                globals
                    .unmodelled_globals
                    .extend(block.unmodelled.iter().cloned());
            }
        }
        // A script reaches the members of a UMD global without an import edge, so it does
        // not wait for the file that exports it. The declarations of that file, and of
        // everything it imports, are therefore handed over here, fixed for the pass, and
        // not read from the tables of whichever file happens to have finished.
        let by_file: FxHashMap<FileId, &UnitExports> = units.iter().copied().collect();
        let mut seen: std::collections::HashSet<FileId> = std::collections::HashSet::new();
        let mut pending: Vec<FileId> = units
            .iter()
            .filter(|(_, unit)| !unit.umd_namespaces.is_empty() || !unit.globals.is_empty())
            .map(|(id, _)| *id)
            .collect();
        while let Some(id) = pending.pop() {
            if !seen.insert(id) {
                continue;
            }
            if let Some(unit) = by_file.get(&id) {
                for decl in &unit.decls {
                    globals.decls.insert(decl.key, Arc::clone(decl));
                }
            }
            pending.extend(graph.dependencies(id).iter().copied());
        }
        globals
    }
}

// The members of a UMD namespace so far, and the file and key of its `export =`.
type NamespaceParts = (Vec<(String, DeclKey)>, Option<(FileId, DeclKey)>);

// UMD aliases are attached to the source file, and their members are derived from its
// export table in this build. `units` is in file order.
fn global_namespaces(units: &[(FileId, &UnitExports)]) -> Vec<GlobalNamespace> {
    let mut namespaces: std::collections::BTreeMap<String, NamespaceParts> =
        std::collections::BTreeMap::new();
    for (file_id, unit) in units {
        let assignment = unit
            .names
            .iter()
            .find(|(name, _)| name == "export=")
            .map(|(_, key)| *key);
        for alias in &unit.umd_namespaces {
            let (members, current_assignment) = namespaces.entry(alias.clone()).or_default();
            members.extend(
                unit.names
                    .iter()
                    .filter(|(name, _)| name != "default" && name != "export=")
                    .cloned(),
            );
            if let Some(key) = assignment {
                if current_assignment.is_none_or(|(current_file, _)| *file_id < current_file) {
                    *current_assignment = Some((*file_id, key));
                }
            }
        }
    }
    namespaces
        .into_iter()
        .map(|(name, (mut members, assignment))| {
            members.sort();
            members.dedup();
            GlobalNamespace {
                name,
                members,
                export_assignment: assignment.map(|(_, key)| key),
            }
        })
        .collect()
}

/// The result of checking one file, as the report and the exports it hands on.
pub(crate) type Checked = (FileReport, Option<UnitExports>);

// What one unit hands back: its files, and the files of a cycle that did not settle.
#[derive(Clone, Default)]
pub(crate) struct UnitResult {
    pub(crate) files: Vec<Checked>,
    pub(crate) unsettled: Vec<FileId>,
}

/// Checks every checkable file of `graph`. The files are the components of the import
/// graph, run as soon as every component they import has finished; a cycle is one unit.
/// The report is sorted by file id, so it does not depend on which thread finished first.
///
/// An import is looked up in the file it names: a file is checked only after everything
/// it imports has been, and what those files export (their annotated interfaces, aliases,
/// functions and constants, as portable declarations) is what its imports bind to. The
/// members of a cycle are checked again against what the previous round exported, until
/// what they export stops changing, or until a bound that grows with the size of the cycle
/// runs out. A cycle that hits the bound is listed in `ProjectReport::unsettled` and its
/// members export as incomplete. An import of a file that is not checked (a dependency's
/// declaration file) binds nothing, which leaves the name unknown: it loses an error and
/// never invents one. The same holds for a file that panicked or was cancelled.
pub fn check_project(graph: &ModuleGraph) -> ProjectReport {
    run_project(graph, None, None, &RunOptions::default())
}

/// `check_project` with the recheck bound of every cycle replaced by `max_cycle_rounds`
/// (`None` keeps the default, see `cycle_round_cap`). Exists so a test can force a cycle
/// to run out of rounds, which no ordinary project does.
#[doc(hidden)]
pub fn check_project_bounded(
    graph: &ModuleGraph,
    max_cycle_rounds: Option<usize>,
) -> ProjectReport {
    run_project(graph, max_cycle_rounds, None, &RunOptions::default())
}

/// `check_project` that stops when `cancel` is cancelled. The token is looked at as each
/// unit starts and inside each file's check; a file that did not run is in the report as a
/// read failure that says so, and `ProjectReport::cancelled` is set, so a cancelled run is
/// never mistaken for a clean one. Files that did run keep their results.
pub fn check_project_cancellable(graph: &ModuleGraph, cancel: &CancelToken) -> ProjectReport {
    run_project(graph, None, Some(cancel), &RunOptions::default())
}

pub(crate) fn run_project(
    graph: &ModuleGraph,
    max_cycle_rounds: Option<usize>,
    cancel: Option<&CancelToken>,
    options: &RunOptions<'_>,
) -> ProjectReport {
    let in_cycle: std::collections::HashSet<FileId> =
        graph.cycles().into_iter().flatten().collect();
    // A file that only declares ambient modules is imported by nobody, so no edge puts it
    // ahead of the files that use what it declares. Those files are checked first.
    let providers = ambient_providers(graph);
    let mut files: Vec<FileReport> = Vec::new();
    let mut unsettled: Vec<FileId> = Vec::new();
    let mut provider_units: Vec<(FileId, UnitExports)> = Vec::new();
    // Whether the memo took the providers; when it did not, it is not asked about the units either.
    let mut memo = options.memo;
    if !providers.is_empty() {
        let project = Project {
            graph,
            tables: RwLock::new(Tables::default()),
            globals: Globals::default(),
            cancel,
            collect_global_decls: true,
            texts: options.texts,
            sequential: false,
        };
        let remembered: Option<Vec<Checked>> = options.memo.and_then(|memo| {
            let ask = || {
                providers
                    .par_iter()
                    .map(|id| memo.provider(*id))
                    .collect::<Option<Vec<Checked>>>()
            };
            match worker_pool() {
                Some(pool) => pool.install(ask),
                None => ask(),
            }
        });
        if remembered.is_none() {
            memo = None;
        }
        let checked = remembered.unwrap_or_else(|| check_files(&project, &providers));
        for (report, unit_exports) in checked {
            if let Some(unit_exports) = unit_exports {
                provider_units.push((report.file_id, unit_exports));
            }
            if !graph.is_dependency_declaration(report.file_id) {
                files.push(report);
            }
        }
        provider_units.sort_by_key(|(id, _)| *id);
        if let Some(memo) = memo {
            memo.providers_ready(&provider_units);
        }
    }
    let provider_refs: Vec<(FileId, &UnitExports)> =
        provider_units.iter().map(|(id, u)| (*id, u)).collect();
    let mut globals = Globals::build(graph, &provider_refs);
    // The wildcard test looks at the providers only, in both passes.
    let wildcard_declared = provider_refs
        .iter()
        .flat_map(|(_, unit)| unit.ambient.iter())
        .any(|module| module.specifier.contains('*'));
    globals.bare_reportable = bare_specifiers_are_reportable(graph, wildcard_declared);
    let provider_set: std::collections::HashSet<FileId> = providers.iter().copied().collect();
    let setup = PassSetup {
        graph,
        providers: &provider_set,
        provider_units: &provider_units,
        in_cycle: &in_cycle,
        max_cycle_rounds,
        cancel,
        texts: options.texts,
        memo,
    };

    #[cfg(test)]
    {
        PASSES_RUN.with(|count| count.set(0));
        FILES_CHECKED.with(|count| count.set(0));
    }
    let first = run_pass(&setup, globals);
    unsettled.extend(first.unsettled.iter().copied());
    // What each file's inferred return types read from imported functions, for the circle
    // check after the passes (see `report_cross_file_returns`). A file the second pass runs
    // again is taken from the second pass.
    let mut return_reads: FxHashMap<FileId, Vec<crate::unit::ReturnRead>> = FxHashMap::default();
    for (id, unit) in provider_units.iter().map(|(id, unit)| (*id, unit)).chain(
        first
            .files
            .iter()
            .filter_map(|(report, unit)| unit.as_ref().map(|u| (report.file_id, u))),
    ) {
        if !unit.return_reads.is_empty() {
            return_reads.insert(id, unit.return_reads.clone());
        }
    }

    // `export as namespace` exposes a module without a static import edge from its
    // consumers, and a module augmentation or an ambient module in a file that is not a
    // provider does not order the files that see it either. So the first pass cannot know
    // every global before checking each file. When the first pass found any, check the
    // non-provider files again against all of them. A project with none has nothing a
    // second pass could change, so the first pass is the answer.
    //
    // A file's global declarations (script files, `declare global`) need a second pass only
    // when some file uses a name another file declares: most projects declare none, and
    // a script file that nothing else reads from would otherwise cost every project a
    // second pass.
    let declared_globals: std::collections::HashSet<&str> = first
        .files
        .iter()
        .filter_map(|(_, unit)| unit.as_ref())
        .flat_map(|unit| unit.globals.iter())
        .flat_map(|block| {
            block
                .export_names
                .iter()
                .map(|(name, _)| name.as_str())
                .chain(block.unmodelled.iter().map(String::as_str))
        })
        .collect();
    // A name two files declare globally is one declaration (two interfaces of one name
    // merge), so each of the files needs the other's half, though neither "uses" the name
    // without declaring it. Counted per file, so a file naming it twice is still one.
    let shared_globals: std::collections::HashSet<&str> = {
        let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for (_, unit) in &first.files {
            let Some(unit) = unit.as_ref() else {
                continue;
            };
            let names: std::collections::HashSet<&str> = unit
                .globals
                .iter()
                .flat_map(|block| {
                    block
                        .export_names
                        .iter()
                        .map(|(name, _)| name.as_str())
                        .chain(block.unmodelled.iter().map(String::as_str))
                })
                .collect();
            for name in names {
                *counts.entry(name).or_default() += 1;
            }
        }
        counts
            .into_iter()
            .filter(|(_, files)| *files > 1)
            .map(|(name, _)| name)
            .collect()
    };
    let wants_second = !shared_globals.is_empty()
        || first.files.iter().any(|(_, unit)| {
            unit.as_ref().is_some_and(|unit| {
                !unit.umd_namespaces.is_empty()
                    || !unit.augmentations.is_empty()
                    || !unit.ambient.is_empty()
                    || unit
                        .unresolved_names
                        .iter()
                        .any(|name| declared_globals.contains(name.as_str()))
            })
        });
    // The files whose answer the second pass can change. The globals the first pass could not
    // see are four kinds, and each reaches a file in a way that can be named:
    //  - a global declaration, or a UMD alias (`export as namespace`), is read by a file that
    //    leaves that name unresolved;
    //  - an ambient module, or an augmentation of a module, is read by a file that imports
    //    that specifier;
    //  - an augmentation also changes what the augmented file exports, so the files that read
    //    from it (added below, once the table is built).
    // Everything else keeps its first result. A wildcard ambient module (`declare module
    // "*.css"`) matches specifiers by pattern, so with one every file is checked again.
    let umd_names: std::collections::HashSet<&str> = first
        .files
        .iter()
        .filter_map(|(_, unit)| unit.as_ref())
        .flat_map(|unit| unit.umd_namespaces.iter().map(String::as_str))
        .collect();
    let module_specifiers: std::collections::HashSet<&str> = first
        .files
        .iter()
        .filter_map(|(_, unit)| unit.as_ref())
        .flat_map(|unit| unit.ambient.iter().chain(unit.augmentations.iter()))
        .map(|module| module.specifier.as_str())
        .collect();
    // Whether any specifier is a pattern; a yes or no, so the set's order cannot reach the answer.
    let wildcard_module = first
        .files
        .iter()
        .filter_map(|(_, unit)| unit.as_ref())
        .flat_map(|unit| unit.ambient.iter().chain(unit.augmentations.iter()))
        .any(|module| module.specifier.contains('*'));
    let mut seeds: std::collections::HashSet<FileId> = first
        .files
        .iter()
        .filter(|(report, unit)| {
            let uses_a_name = unit.as_ref().is_some_and(|unit| {
                unit.unresolved_names.iter().any(|name| {
                    declared_globals.contains(name.as_str()) || umd_names.contains(name.as_str())
                }) || unit.globals.iter().any(|block| {
                    block
                        .export_names
                        .iter()
                        .map(|(name, _)| name.as_str())
                        .chain(block.unmodelled.iter().map(String::as_str))
                        .any(|name| shared_globals.contains(name))
                })
            });
            uses_a_name
                || graph
                    .edges(report.file_id)
                    .iter()
                    .any(|edge| module_specifiers.contains(edge.specifier.as_str()))
        })
        .map(|(report, _)| report.file_id)
        .collect();
    drop(declared_globals);
    // A token cancelled after the first pass finished leaves every file with a result, so
    // the per-file failure below would not show it, yet those results were computed without
    // the globals the second pass exists to supply. Counting the skipped pass as a
    // cancellation keeps such a run from reading as a clean one.
    let skipped_second = wants_second && cancel.is_some_and(CancelToken::is_cancelled);
    let needs_second = wants_second && !skipped_second;
    let (last, kept) = if needs_second {
        // A script lists only its global names in the first pass. The ones whose names some
        // other file uses are checked again on their own, asked for their declarations: a
        // script imports nothing, so it needs no other file's exports to do so. The rest keep
        // their name list, which nothing reads.
        let used_names: std::collections::HashSet<&str> = first
            .files
            .iter()
            .filter_map(|(_, unit)| unit.as_ref())
            .flat_map(|unit| unit.unresolved_names.iter().map(String::as_str))
            // Collected into a set, so the order it is read in cannot reach a result.
            .chain(
                #[allow(clippy::disallowed_methods)]
                shared_globals.iter().copied(),
            )
            .collect();
        let script_sources: Vec<FileId> = first
            .files
            .iter()
            .filter(|(_, unit)| {
                unit.as_ref().is_some_and(|unit| {
                    unit.globals.iter().any(|block| {
                        block.names_only
                            && block
                                .unmodelled
                                .iter()
                                .any(|n| used_names.contains(n.as_str()))
                    })
                })
            })
            .map(|(report, _)| report.file_id)
            .collect();
        drop(used_names);
        let mut rebuilt: Vec<(FileId, UnitExports)> = Vec::new();
        if !script_sources.is_empty() {
            let scripts = Project {
                graph,
                tables: RwLock::new(Tables::default()),
                globals: Globals::default(),
                cancel,
                collect_global_decls: true,
                texts: options.texts,
                sequential: false,
            };
            for (report, unit_exports) in check_files(&scripts, &script_sources) {
                if let Some(unit_exports) = unit_exports {
                    rebuilt.push((report.file_id, unit_exports));
                }
            }
        }
        let rebuilt_ids: std::collections::HashSet<FileId> =
            rebuilt.iter().map(|(id, _)| *id).collect();
        let mut all: Vec<(FileId, &UnitExports)> = provider_refs.clone();
        all.extend(
            first
                .files
                .iter()
                .filter(|(report, _)| !rebuilt_ids.contains(&report.file_id))
                .filter_map(|(report, unit)| unit.as_ref().map(|u| (report.file_id, u))),
        );
        all.extend(rebuilt.iter().map(|(id, unit)| (*id, unit)));
        all.sort_by_key(|(id, _)| *id);
        let mut second_globals = Globals::build(graph, &all);
        second_globals.bare_reportable = bare_specifiers_are_reportable(graph, wildcard_declared);
        // Problem: the second pass checked every file again, though a file that uses none of the
        // names the project declares globally, and imports nothing that does, cannot come out
        // different.
        // Picked: when global declarations are the only reason for a second pass, only the users
        // of those names and every file that reads from them (importers, directly or through
        // others, and files that load them lazily) are checked again. The rest keep their first
        // result and are treated like providers: published up front, not run. A file's result
        // depends on the globals only through the names it leaves unresolved, so this is the same
        // answer as checking everything again.
        // Cost: the files kept from the first pass stay in memory next to the second pass's.
        // What reads from an augmented file sees the augmentation in what that file exports.
        // The targets go into a set of seeds, so the order they are read in cannot reach a result.
        #[allow(clippy::disallowed_methods)]
        let targets: Vec<FileId> = second_globals.augmentations.keys().copied().collect();
        for target in &targets {
            seeds.extend(
                graph
                    .dependents(*target)
                    .iter()
                    .chain(graph.dynamic_dependents(*target))
                    .copied(),
            );
        }
        let affected: Option<std::collections::HashSet<FileId>> =
            if wildcard_module || force_full_second() {
                None
            } else {
                Some(affected_files(graph, &seeds))
            };
        let mut providers2 = provider_set.clone();
        let mut units2: Vec<(FileId, UnitExports)> = provider_units.clone();
        let mut kept: Vec<FileReport> = Vec::new();
        if let Some(affected) = &affected {
            for (report, unit) in first.files {
                if affected.contains(&report.file_id) {
                    continue;
                }
                providers2.insert(report.file_id);
                if let Some(unit) = unit {
                    units2.push((report.file_id, unit));
                }
                kept.push(report);
            }
        }
        units2.sort_by_key(|(id, _)| *id);
        let setup2 = PassSetup {
            graph,
            providers: &providers2,
            provider_units: &units2,
            in_cycle: &in_cycle,
            max_cycle_rounds,
            cancel,
            texts: options.texts,
            memo: None,
        };
        // The second pass is answered from the memo when everything it reads is what it read
        // last time: the first pass's outputs (by their digests), and the text of each file it
        // checks again. Without this a project with a shared global declaration checked its
        // affected files again on every check, even an unchanged one.
        let fingerprint = match (memo, options.texts) {
            (Some(memo), Some(texts)) => second_pass_fingerprint(
                memo,
                graph,
                texts,
                affected.as_ref(),
                &script_sources,
                wildcard_declared,
            ),
            _ => None,
        };
        let second = match (memo, fingerprint) {
            (Some(memo), Some(fingerprint)) => match memo.second_pass(fingerprint) {
                Some(kept_result) => kept_result,
                None => {
                    let result = run_pass(&setup2, second_globals);
                    let stopped = cancel.is_some_and(CancelToken::is_cancelled);
                    if !stopped && second_pass_is_keepable(&result) {
                        memo.keep_second_pass(fingerprint, &result);
                    }
                    result
                }
            },
            _ => run_pass(&setup2, second_globals),
        };
        unsettled.extend(second.unsettled.iter().copied());
        for (report, unit) in &second.files {
            match unit.as_ref().filter(|u| !u.return_reads.is_empty()) {
                Some(unit) => {
                    return_reads.insert(report.file_id, unit.return_reads.clone());
                }
                None => {
                    return_reads.remove(&report.file_id);
                }
            }
        }
        (second, kept)
    } else {
        (first, Vec::new())
    };
    for report in kept {
        if !graph.is_dependency_declaration(report.file_id) {
            files.push(report);
        }
    }
    for (report, _) in last.files {
        // A package's declaration file is read for what it exports and never reported.
        if !graph.is_dependency_declaration(report.file_id) {
            files.push(report);
        }
    }
    report_cross_file_returns(graph, &return_reads, &mut files);
    files.sort_by_key(|report| report.file_id);
    unsettled.sort();
    unsettled.dedup();
    let cancelled = skipped_second
        || files.iter().any(
            |report| matches!(&report.outcome, FileOutcome::ReadFailed(text) if text == CANCELLED),
        );
    ProjectReport {
        files,
        unsettled,
        cancelled,
    }
}

// Problem: two functions with no return annotation, in two files, whose return types are each
// read from the other have no answer, and tsc reports TS7023 on both. A single file only sees
// its own calls, so each file read the other's function as `any` without a word.
// Picked: every file hands over, for each exported unannotated function, the imported
// functions its return expressions reach (through its own functions too). Here the nodes are
// those functions and an edge is such a read; a component of more than one function is a
// circle, and each member is reported where it is declared, unless its own file already
// reported it as part of a circle of its own.
// Cost: a function that is not exported, though it lies on the circle, is not a node and is
// not reported (tsc reports it); a call through a method or a variable is not read.
fn report_cross_file_returns(
    graph: &ModuleGraph,
    reads: &FxHashMap<FileId, Vec<crate::unit::ReturnRead>>,
    files: &mut [FileReport],
) {
    // Sorted straight away, so the order of the map does not reach the report.
    #[allow(clippy::disallowed_methods)]
    let mut ids: Vec<FileId> = reads.keys().copied().collect();
    ids.sort();
    let nodes: Vec<(FileId, &crate::unit::ReturnRead)> = ids
        .iter()
        .flat_map(|id| reads[id].iter().map(move |read| (*id, read)))
        .collect();
    if nodes.is_empty() {
        return;
    }
    let index: FxHashMap<DeclKey, usize> = nodes
        .iter()
        .enumerate()
        .map(|(position, (_, read))| (read.owner, position))
        .collect();
    let adjacency: Vec<Vec<usize>> = nodes
        .iter()
        .map(|(_, read)| {
            let mut targets: Vec<usize> = read
                .callees
                .iter()
                .filter_map(|key| index.get(key).copied())
                .collect();
            targets.sort_unstable();
            targets.dedup();
            targets
        })
        .collect();
    for component in crate::scc::components(&adjacency) {
        if component.len() < 2 {
            continue;
        }
        for position in component {
            let (file, read) = nodes[position];
            if read.reported_locally {
                continue;
            }
            let Some(report) = files.iter_mut().find(|report| report.file_id == file) else {
                continue;
            };
            let FileOutcome::Checked(diagnostics) = &mut report.outcome else {
                continue;
            };
            let message = crate::diagnostic_messages::messages::return_implicitly_any(&read.name);
            diagnostics.push(Diagnostic {
                severity: crate::diagnostics::Severity::Error,
                code: message.code,
                message: message.text,
                file_name: graph
                    .files()
                    .path(file)
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                start: read.start,
                end: read.end,
            });
            diagnostics.sort_by_key(|diagnostic| diagnostic.start);
        }
    }
}

// The files whose result can change when what `seeds` see changes: the seeds and everything that
// reads from them, through imports or lazy imports, followed to the end.
fn affected_files(
    graph: &ModuleGraph,
    seeds: &std::collections::HashSet<FileId>,
) -> std::collections::HashSet<FileId> {
    let mut seen = seeds.clone();
    // The walk fills a set, so the order the seeds are taken in cannot reach a result.
    #[allow(clippy::disallowed_methods)]
    let mut stack: Vec<FileId> = seeds.iter().copied().collect();
    while let Some(id) = stack.pop() {
        for dependent in graph
            .dependents(id)
            .iter()
            .chain(graph.dynamic_dependents(id))
        {
            if seen.insert(*dependent) {
                stack.push(*dependent);
            }
        }
    }
    seen
}

#[cfg(not(test))]
fn force_full_second() -> bool {
    false
}

#[cfg(test)]
fn force_full_second() -> bool {
    FORCE_FULL_SECOND.with(|flag| flag.get())
}

// Test hook: make the second pass check every file, to compare it with the narrowed one.
#[cfg(test)]
thread_local! {
    static FORCE_FULL_SECOND: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static FILES_CHECKED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

// Test hook: how many passes this thread's last `check_project` ran. Thread-local, because
// the passes are started from the calling thread and tests run in parallel.
#[cfg(test)]
thread_local! {
    static PASSES_RUN: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

struct PassSetup<'a> {
    graph: &'a ModuleGraph,
    providers: &'a std::collections::HashSet<FileId>,
    provider_units: &'a [(FileId, UnitExports)],
    in_cycle: &'a std::collections::HashSet<FileId>,
    max_cycle_rounds: Option<usize>,
    cancel: Option<&'a CancelToken>,
    texts: Option<&'a TextFn>,
    // Answers units of the first pass only; None for the second.
    memo: Option<&'a dyn UnitMemo>,
}

// One pass over every component of the graph, on the ready queue. The files come back in
// component order, which is fixed by the graph, and every file of a component that did not
// finish is in the list as a failure, so none is left out.
fn run_pass(setup: &PassSetup<'_>, globals: Globals) -> UnitResult {
    #[cfg(test)]
    PASSES_RUN.with(|count| count.set(count.get() + 1));
    let project = Project {
        graph: setup.graph,
        tables: RwLock::new(Tables::default()),
        globals,
        cancel: setup.cancel,
        collect_global_decls: false,
        texts: setup.texts,
        sequential: false,
    };
    for (id, unit) in setup.provider_units {
        project.publish(*id, unit.clone());
    }
    let waits = unit_dependencies(setup.graph);
    let outcomes = run_units(worker_pool(), &waits, setup.cancel, |unit, _| {
        run_unit(&project, setup, unit)
    })
    .expect("the components of a graph never wait on each other");
    let mut result = UnitResult::default();
    for (unit, outcome) in outcomes.into_iter().enumerate() {
        match outcome {
            UnitOutcome::Done(done) => {
                result.files.extend(done.files);
                result.unsettled.extend(done.unsettled);
            }
            UnitOutcome::Panicked => {
                for id in unit_files(setup, unit) {
                    let mut failed =
                        failed_report(id, "internal error: checking this file panicked");
                    failed.incomplete_reasons = vec![IncompleteReason::CheckerCrashed];
                    result.files.push((failed, None));
                }
            }
            UnitOutcome::Cancelled => {
                for id in unit_files(setup, unit) {
                    result.files.push((failed_report(id, CANCELLED), None));
                }
            }
        }
    }
    #[cfg(test)]
    FILES_CHECKED.with(|count| count.set(count.get() + result.files.len()));
    result
}

fn failed_report(id: FileId, text: &str) -> FileReport {
    FileReport {
        file_id: id,
        outcome: FileOutcome::ReadFailed(text.to_string()),
        reads: Vec::new(),
        incomplete_reasons: Vec::new(),
    }
}

// The files of a component that this pass checks: the checkable ones, and a package's
// declaration files, without the providers (those ran before the pass).
fn unit_files(setup: &PassSetup<'_>, unit: usize) -> Vec<FileId> {
    setup.graph.sccs()[unit]
        .iter()
        .copied()
        .filter(|id| {
            !setup.providers.contains(id)
                && (setup.graph.is_checkable(*id) || setup.graph.is_dependency_declaration(*id))
        })
        .collect()
}

fn run_unit(project: &Project<'_>, setup: &PassSetup<'_>, unit: usize) -> UnitResult {
    let members = unit_files(setup, unit);
    if members.is_empty() {
        return UnitResult::default();
    }
    if let Some(memo) = setup.memo
        && let Some(done) = memo.unit(unit)
    {
        for (report, unit_exports) in &done.files {
            if let Some(unit_exports) = unit_exports {
                project.publish(report.file_id, unit_exports.clone());
            }
        }
        return done;
    }

    let cyclic = setup.graph.sccs()[unit]
        .iter()
        .any(|id| setup.in_cycle.contains(id));
    if !cyclic {
        let id = members[0];
        let (report, unit_exports) = with_worker(|worker| check_file(project, worker, id));
        if let Some(unit_exports) = &unit_exports {
            project.publish(id, unit_exports.clone());
        }
        return UnitResult {
            files: vec![(report, unit_exports)],
            unsettled: Vec::new(),
        };
    }
    let mut done = check_files(project, &members);
    let mut unsettled = Vec::new();
    let rounds = setup
        .max_cycle_rounds
        .unwrap_or_else(|| cycle_round_cap(members.len()));
    if !settle_cycle(project, &members, &mut done, rounds) {
        for (report, unit_exports) in done.iter_mut() {
            // The file is named as incomplete, not only its export list flagged, so a
            // report of partial files can say why and count it.
            let reason = IncompleteReason::CycleDidNotSettle;
            if !report.incomplete_reasons.contains(&reason) {
                report.incomplete_reasons.push(reason);
            }
            if let Some(unit_exports) = unit_exports {
                unit_exports.complete = false;
            }
        }
        unsettled.extend(members.iter().copied());
    }
    for (report, unit_exports) in &done {
        if let Some(unit_exports) = unit_exports {
            project.publish(report.file_id, unit_exports.clone());
        }
    }
    UnitResult {
        files: done,
        unsettled,
    }
}

/// Checks one component of `graph` against what its dependencies exported, and nothing else:
/// the first pass of `run_project` for one unit, without the other units. `dependencies` must
/// hold every unit the component reads from, directly or through declarations it names.
/// The members of a cycle are checked one after another, on this thread.
#[cfg(feature = "incremental")]
pub(crate) fn check_unit_isolated(
    graph: &ModuleGraph,
    unit: usize,
    in_cycle: &std::collections::HashSet<FileId>,
    dependencies: &[(FileId, &UnitExports)],
    texts: &TextFn,
    cancel: Option<&CancelToken>,
    provider_units: &[(FileId, UnitExports)],
) -> UnitResult {
    // The same globals `run_project` builds for its first pass: what the ambient providers
    // declared, and whether a missing package is reportable given a wildcard module they declare.
    let provider_refs: Vec<(FileId, &UnitExports)> =
        provider_units.iter().map(|(id, u)| (*id, u)).collect();
    let mut globals = Globals::build(graph, &provider_refs);
    let wildcard_declared = provider_refs
        .iter()
        .flat_map(|(_, unit)| unit.ambient.iter())
        .any(|module| module.specifier.contains('*'));
    globals.bare_reportable = bare_specifiers_are_reportable(graph, wildcard_declared);
    let project = Project {
        graph,
        tables: RwLock::new(Tables::default()),
        globals,
        cancel,
        collect_global_decls: false,
        texts: Some(texts),
        sequential: true,
    };
    for (id, unit) in provider_units {
        project.publish(*id, unit.clone());
    }
    for (id, exports) in dependencies {
        project.publish_ref(*id, exports);
    }
    let providers: std::collections::HashSet<FileId> =
        provider_units.iter().map(|(id, _)| *id).collect();
    let setup = PassSetup {
        graph,
        providers: &providers,
        provider_units,
        in_cycle,
        max_cycle_rounds: None,
        cancel,
        texts: Some(texts),
        memo: None,
    };
    run_unit(&project, &setup, unit)
}

/// Everything a unit reads from the graph besides its own edges and its dependencies' exports:
/// the compiler options and whether a missing package is reported. Two graphs with the same
/// digest give a unit the same environment.
#[cfg(feature = "incremental")]
pub(crate) fn unit_environment(graph: &ModuleGraph) -> String {
    format!(
        "{:?}|{}|{:?}|{}",
        graph.check_options(),
        bare_specifiers_are_reportable(graph, false),
        graph.root(),
        graph.type_entries_loaded(),
    )
}

/// The ambient providers of a graph: the files `run_project` checks before the first pass.
#[cfg(feature = "incremental")]
pub(crate) fn provider_ids(graph: &ModuleGraph) -> Vec<FileId> {
    ambient_providers(graph)
}

/// Checks one ambient provider the way `run_project` does before its first pass: against
/// nothing, collecting the global declarations it makes.
#[cfg(feature = "incremental")]
pub(crate) fn check_provider_isolated(
    graph: &ModuleGraph,
    id: FileId,
    texts: &TextFn,
    cancel: Option<&CancelToken>,
) -> Checked {
    let project = Project {
        graph,
        tables: RwLock::new(Tables::default()),
        globals: Globals::default(),
        cancel,
        collect_global_decls: true,
        texts: Some(texts),
        sequential: true,
    };
    with_worker(|worker| check_file(&project, worker, id))
}

thread_local! {
    // One worker per thread, so its arena is reused from file to file; the worker clears
    // it after every unit, so nothing carries over.
    static WORKER: std::cell::RefCell<Worker> = std::cell::RefCell::new(Worker::new());
}

fn with_worker<T>(run: impl FnOnce(&mut Worker) -> T) -> T {
    WORKER.with(|cell| match cell.try_borrow_mut() {
        Ok(mut worker) => run(&mut worker),
        // Only if a unit were started while this thread was inside another; a fresh worker
        // is slower and correct.
        Err(_) => run(&mut Worker::new()),
    })
}

// Whether a package specifier that no file answers (`import x from \"react\"`) can be called
// a missing module. TypeScript also finds one through `paths` or `baseUrl` (possibly set in
// an `extends`ed config) and `typeRoots`, through a `declare module` pattern
// (`\"*.css\"`), and through any `@types` package, whose declarations are global and are
// not files this project imports. A project with none of those has nothing else to
// answer it, so the specifier really is missing; with any of them it is left alone.
fn bare_specifiers_are_reportable(graph: &ModuleGraph, wildcard_module_declared: bool) -> bool {
    let Some(root) = graph.root() else {
        return false;
    };
    if wildcard_module_declared {
        return false;
    }
    // With the configuration attached, the effective options decide, after `extends`: a
    // `paths` or `baseUrl` the resolver applied has already had its say, so only an option
    // the checker does not apply (`typeRoots`) can still hide a package.
    let parsed = graph.config_hides_bare_specifiers();
    if parsed == Some(true) {
        return false;
    }
    // Without it, the config can only be searched as text. `paths` and `baseUrl` count only
    // when the resolver did not apply them, and `extends` always counts, because the
    // extended file can hold any of the rest.
    // `customConditions` and `moduleSuffixes` are never applied by the resolver, so they
    // count either way.
    let loaded = graph.type_entries_loaded();
    let mut keys: Vec<&str> = vec![
        "\"rootDirs\"",
        "\"extends\"",
        "\"customConditions\"",
        "\"moduleSuffixes\"",
    ];
    // `typeRoots` holds a package back only while the declarations in it are not read.
    if !loaded {
        keys.push("\"typeRoots\"");
    }
    if !graph.tsconfig_applied() {
        keys.extend(["\"paths\"", "\"baseUrl\""]);
    }
    for directory in root.ancestors() {
        // An `@types` package is global, so it can answer a name no import reaches, unless the
        // declarations of those packages were loaded and are known.
        if !loaded && directory.join("node_modules").join("@types").is_dir() {
            return false;
        }
        if parsed.is_some() {
            continue;
        }
        for name in ["tsconfig.json", "jsconfig.json"] {
            if let Ok(text) = fs::read_to_string(directory.join(name)) {
                if keys.iter().any(|key| text.contains(*key)) {
                    return false;
                }
            }
        }
    }
    true
}

// A specifier that is not a package name to look up: a Node built-in (`fs`, `node:path`),
// anything with a scheme or an absolute path, and a `#` import of the package's own map.
fn is_special_specifier(specifier: &str) -> bool {
    const BUILTINS: &[&str] = &[
        "assert",
        "buffer",
        "child_process",
        "cluster",
        "console",
        "constants",
        "crypto",
        "dgram",
        "dns",
        "domain",
        "events",
        "fs",
        "http",
        "http2",
        "https",
        "inspector",
        "module",
        "net",
        "os",
        "path",
        "perf_hooks",
        "process",
        "punycode",
        "querystring",
        "readline",
        "repl",
        "stream",
        "string_decoder",
        "sys",
        "timers",
        "tls",
        "trace_events",
        "tty",
        "url",
        "util",
        "v8",
        "vm",
        "wasi",
        "worker_threads",
        "zlib",
    ];
    if specifier.contains(':') || specifier.starts_with('/') || specifier.starts_with('#') {
        return true;
    }
    let first = specifier.split('/').next().unwrap_or(specifier);
    BUILTINS.contains(&first)
}

// Declaration files that declare ambient modules and are scripts rather than external
// modules. Discovery already parsed every file to find its imports, and it recorded which
// scripts declare ambient modules, so nothing is read or parsed again here. These
// providers are checked before consumers so their portable exports are available even
// though they do not have ordinary import edges.
fn ambient_providers(graph: &ModuleGraph) -> Vec<FileId> {
    let mut found = Vec::new();
    for id in graph.sccs().iter().flatten().copied() {
        if !(graph.is_checkable(id) || graph.is_dependency_declaration(id)) {
            continue;
        }
        let Some(path) = graph.files().path(id) else {
            continue;
        };
        let is_declaration = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                name.ends_with(".d.ts") || name.ends_with(".d.mts") || name.ends_with(".d.cts")
            });
        if is_declaration && graph.declares_ambient_modules(id) {
            found.push(id);
        }
    }
    found.sort();
    found
}

// Small cycles keep the old low recheck cost. Longer cycles need more rounds because an
// export can travel only one re-export edge per snapshot: a value declared near one end
// of a ring may take O(cycle length) passes to reach the other end. The extra pass lets
// us observe that the exports have stopped changing. This remains a safety bound, not a
// proof of convergence for arbitrary inferred-signature cycles.
const MIN_CYCLE_ROUNDS: usize = 4;

// How many rechecks a cycle of `members` files gets: one per member plus a spare, so the
// longest chain of re-exports can finish travelling and one more round can see that
// nothing changed, and never fewer than MIN_CYCLE_ROUNDS.
fn cycle_round_cap(members: usize) -> usize {
    members.saturating_add(1).max(MIN_CYCLE_ROUNDS)
}

// Stack for each file-checking worker thread.
//
// Problem: a project run aborted with a stack overflow on a file that is a single
// chain of about 1,500 `+` operators (TypeScript's own binderBinaryExpressionStress).
// Such a chain parses into a left-nested tree 1,500 levels deep, and walking it
// recurses once per level inside oxc's visitors (oxc_ast_visit's walk_expression
// and walk_binary_expression, 16,000 frames in the backtrace), which is not our
// code to make iterative. The same file passes when it is the only one checked,
// because that runs on the main thread, whose stack is about 8 MB. Rayon's
// workers are spawned threads, and a spawned thread gets about 2 MB by default.
// Picked: build the workers with a stack that is comfortably deeper than any file
// we have seen, 64 MB. It is address space that is only touched where a file
// really recurses deeply, so ordinary runs do not use more memory.
// Cost: a chain several times longer than this one would still overflow. Fixing
// that for good means a depth limit in the parser or an iterative walk, which is
// oxc's to provide; this only removes the gap between the main thread and the
// workers. The old workaround was to export RUST_MIN_STACK before running.
const WORKER_STACK_BYTES: usize = crate::stack_guard::BIG_STACK_BYTES;

// Determinism stress hook. Compiled only into unit tests: it changes completion order
// without changing checker inputs, so a test can prove reports do not depend on which
// worker finishes first. Production builds have no delay branch or environment knob.
#[cfg(test)]
static TEST_DELAY_SALT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

// The pool is built once and kept, since each worker owns an arena that is reused
// from file to file. If the threads cannot be created, None sends the caller to
// rayon's global pool, which works but has the small default stack.
fn worker_pool() -> Option<&'static rayon::ThreadPool> {
    static POOL: OnceLock<Option<rayon::ThreadPool>> = OnceLock::new();
    POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .stack_size(WORKER_STACK_BYTES)
            .start_handler(|_| crate::stack_guard::set_thread_stack(WORKER_STACK_BYTES))
            .build()
            .ok()
    })
    .as_ref()
}

// Checks `ids` in parallel, against the tables as they stand. Used for the providers, which
// have nothing to wait for, and for the rounds of one cycle.
fn check_files(project: &Project<'_>, ids: &[FileId]) -> Vec<Checked> {
    let graph = project.graph;
    if project.sequential {
        return ids
            .iter()
            .filter(|id| graph.is_checkable(**id) || graph.is_dependency_declaration(**id))
            .map(|id| with_worker(|worker| check_file(project, worker, *id)))
            .collect();
    }
    let check_layer = || {
        ids.par_iter()
            .filter(|id| graph.is_checkable(**id) || graph.is_dependency_declaration(**id))
            .map_init(Worker::new, |worker, id| check_file(project, worker, *id))
            .collect()
    };
    match worker_pool() {
        Some(pool) => pool.install(check_layer),
        None => check_layer(),
    }
}

// Members of a cycle import each other, so the first pass may find nothing among them.
// Each subsequent pass sees the previous pass's exports. Rechecking stops at a stable
// fingerprint or after `max_rounds` passes. Every pass reads one snapshot, so the result
// does not depend on thread timing. `done` holds the members' latest results.
//
// Returns whether the exports settled. False means the bound ran out while they were still
// changing, which the caller reports instead of treating the last pass as final.
fn settle_cycle(
    project: &Project<'_>,
    members: &[FileId],
    done: &mut [Checked],
    max_rounds: usize,
) -> bool {
    // Each round checks all members against one snapshot from the previous round. In a
    // long ring, star-re-exported names may need one round per edge before the fingerprint
    // can stabilize, which is why the caller's bound grows with the cycle.
    for _ in 0..max_rounds {
        let before = fingerprint(done, members);
        for (report, unit_exports) in done.iter() {
            if let Some(unit_exports) = unit_exports {
                if members.contains(&report.file_id) {
                    project.publish(report.file_id, unit_exports.clone());
                }
            }
        }
        let again = check_files(project, members);
        let settled = fingerprint(&again, members) == before;
        for (report, unit_exports) in again {
            if let Some(slot) = done.iter_mut().find(|(r, _)| r.file_id == report.file_id) {
                *slot = (report, unit_exports);
            }
        }
        if settled {
            return true;
        }
    }
    false
}

// What the members export, in a form two passes can be compared by.
fn fingerprint(
    done: &[(FileReport, Option<UnitExports>)],
    members: &[FileId],
) -> Vec<(
    FileId,
    bool,
    Arc<Vec<(String, DeclKey)>>,
    Vec<crate::keys::SigHash>,
)> {
    let mut seen: Vec<_> = done
        .iter()
        .filter(|(report, _)| members.contains(&report.file_id))
        .filter_map(|(report, unit)| {
            unit.as_ref().map(|unit| {
                (
                    report.file_id,
                    unit.complete,
                    unit.names.clone(),
                    unit.decls.iter().map(|decl| decl.sig).collect(),
                )
            })
        })
        .collect();
    seen.sort_by_key(|entry| entry.0);
    seen
}

// What one checked file hands to the files that import it.
#[derive(Clone, Debug)]
pub(crate) struct UnitExports {
    // Shared, so a unit that reads many dependencies holds handles, not copies.
    pub(crate) names: Arc<Vec<(String, DeclKey)>>,
    pub(crate) decls: Vec<Arc<PortableDecl>>,
    // Whether `names` is the module's whole export list (see UnitOutput).
    pub(crate) complete: bool,
    // The modules the file declares with `declare module "x"`.
    pub(crate) ambient: Vec<crate::unit::AmbientExports>,
    pub(crate) augmentations: Vec<crate::unit::AmbientExports>,
    pub(crate) umd_namespaces: Vec<String>,
    // The names the file uses that nothing in it declares.
    pub(crate) unresolved_names: Vec<String>,
    // What the file declares globally (see UnitOutput::globals).
    pub(crate) globals: Vec<crate::unit::AmbientExports>,
    // Exported unannotated functions that read imported ones (see UnitOutput::return_reads).
    pub(crate) return_reads: Vec<crate::unit::ReturnRead>,
}

// The imports of one file: its specifiers are resolved through the graph's own edges, so
// the resolution the graph already did (and cached) is the one used here.
struct ImportsOf<'a> {
    project: &'a Project<'a>,
    from: FileId,
    // Every declaration the unit asked for, in the order it asked. A unit runs on one
    // thread, so the lock is never contended; it is there because the provider is
    // handed out as a shared reference.
    reads: std::sync::Mutex<Vec<DeclKey>>,
}

impl ImportsOf<'_> {
    fn take_reads(&self) -> Vec<DeclKey> {
        let mut reads = std::mem::take(&mut *self.reads.lock().unwrap_or_else(|e| e.into_inner()));
        reads.sort();
        reads.dedup();
        reads
    }

    // Problem: a dynamic `import("x")` does not order the graph (lazy imports may form cycles),
    // so a unit that only imports `x` dynamically did not wait for it. Reading its exports then
    // gave a different answer depending on whether `x` happened to have finished: typed in
    // one run, unknown in the next.
    // Picked: the edge a specifier is read through is a waited-for one when the file has any (a
    // static import, an `import type`, an import type). A dynamic-only edge is read when
    // `ModuleGraph::ordered_dynamic` lists it, that is, when waiting for it cannot make a cycle;
    // the check waits for those units, so the answer is the same in every run.
    // Cost: a lazy import that would close a cycle (`a` loads `b`, `b` imports `a`) stays
    // `Promise<any>`, and so does one of two files that load each other.
    fn edge_for(&self, specifier: &str) -> Option<&super::ModuleEdge> {
        let edges = self.project.graph.edges(self.from);
        edges
            .iter()
            .find(|edge| {
                edge.specifier == specifier && edge.kind != super::ModuleRequestKind::DynamicImport
            })
            .or_else(|| edges.iter().find(|edge| edge.specifier == specifier))
    }

    // A dynamic import this unit does not wait for, so what the target exports cannot be read.
    fn is_unordered_dynamic(&self, edge: &super::ModuleEdge) -> bool {
        edge.kind == super::ModuleRequestKind::DynamicImport
            && !edge.target.is_some_and(|target| {
                self.project
                    .graph
                    .ordered_dynamic(self.from)
                    .contains(&target)
            })
    }

    fn target(&self, specifier: &str) -> Option<FileId> {
        let edge = self.edge_for(specifier)?;
        if self.is_unordered_dynamic(edge) {
            return None;
        }
        edge.target
    }

    // Problem: an augmentation adds members to an interface or class the module declares, so
    // on its own it is only part of that declaration. When the module's own exports are not
    // known in full (a package file with no name and version to key it by), the base
    // declaration is missing and the augmentation alone stood in for the whole type: every
    // member the module really declares was then reported as not existing.
    // Picked: an augmentation of an interface or class is used only when the module's list
    // has that name already, or is known in full. A function or constant it adds stands alone.
    // Cost: an augmented interface of a module we cannot read stays unresolved, silently.
    fn augmentation_keys(
        &self,
        target: FileId,
        base: &[(String, DeclKey)],
    ) -> Vec<(String, DeclKey)> {
        let Some((extra, _)) = self.project.globals.augmentations.get(&target) else {
            return Vec::new();
        };
        let base_complete = self
            .project
            .read()
            .complete
            .get(&target)
            .copied()
            .unwrap_or(false);
        extra
            .iter()
            .filter(|(name, key)| {
                if base_complete || base.iter().any(|(existing, _)| existing == name) {
                    return true;
                }
                let merges =
                    self.project.globals.decls.get(key).is_some_and(|decl| {
                        matches!(decl.kind, DeclKind::Interface | DeclKind::Class)
                    });
                !merges
            })
            .cloned()
            .collect()
    }

    fn append_augmentations(&self, target: FileId, names: &mut Vec<(String, DeclKey)>) {
        let extra = self.augmentation_keys(target, names);
        names.extend(extra);
        names.sort();
        names.dedup();
    }
}

impl DeclProvider for ImportsOf<'_> {
    fn decl(&self, key: DeclKey) -> Option<Arc<PortableDecl>> {
        self.reads
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(key);
        if let Some(decl) = self.project.read().decls.get(&key) {
            return Some(decl.clone());
        }
        self.project.globals.decls.get(&key).cloned()
    }
}

impl ImportSource for ImportsOf<'_> {
    fn export_all(&self, specifier: &str) -> Vec<(String, DeclKey)> {
        // A `declare module "x"` answers `x` before any file does, as it does in TypeScript.
        if let Some((names, _)) = self.project.globals.ambient.get(specifier) {
            return names
                .iter()
                .filter(|(name, _)| name != "default")
                .cloned()
                .collect();
        }
        let Some(target) = self.target(specifier) else {
            return Vec::new();
        };
        let mut names = self
            .project
            .read()
            .names
            .get(&target)
            .into_iter()
            .flat_map(|names| names.iter())
            .filter(|(name, _)| name != "default")
            .cloned()
            .collect::<Vec<_>>();
        self.append_augmentations(target, &mut names);
        names
            .into_iter()
            .filter(|(name, _)| name != "default")
            .collect()
    }

    fn module_exports(&self, specifier: &str) -> ModuleExports {
        let globals = &self.project.globals;
        if let Some((_, complete)) = globals.ambient.get(specifier) {
            return ModuleExports::Known {
                complete: *complete,
            };
        }
        let Some(edge) = self.edge_for(specifier) else {
            return ModuleExports::Unknown;
        };
        let dynamic_only = self.is_unordered_dynamic(edge);
        let Some(target) = edge.target else {
            // A relative specifier no file answers is missing. A package specifier is only
            // known to be missing when the project's configuration cannot be hiding it.
            let relative = specifier.starts_with('.');
            return if relative || (globals.bare_reportable && !is_special_specifier(specifier)) {
                ModuleExports::Unresolved
            } else {
                ModuleExports::Unknown
            };
        };
        if dynamic_only {
            return ModuleExports::Unknown;
        }
        match self.project.read().complete.get(&target) {
            Some(complete) => ModuleExports::Known {
                complete: *complete
                    && globals
                        .augmentations
                        .get(&target)
                        .is_none_or(|(_, augmentation_complete)| *augmentation_complete),
            },
            None => ModuleExports::Unknown,
        }
    }

    fn export_keys(&self, specifier: &str, name: &str) -> Vec<DeclKey> {
        if let Some((names, _)) = self.project.globals.ambient.get(specifier) {
            return names
                .iter()
                .filter(|(exported, _)| exported == name)
                .map(|(_, key)| *key)
                .collect();
        }
        let Some(target) = self.target(specifier) else {
            return Vec::new();
        };
        let mut names = self
            .project
            .read()
            .names
            .get(&target)
            .into_iter()
            .flat_map(|names| names.iter())
            .filter(|(exported, _)| exported == name)
            .map(|(_, key)| *key)
            .collect::<Vec<_>>();
        let base: Vec<(String, DeclKey)> =
            names.iter().map(|key| (name.to_owned(), *key)).collect();
        names.extend(
            self.augmentation_keys(target, &base)
                .into_iter()
                .filter(|(exported, _)| exported == name)
                .map(|(_, key)| key),
        );
        names.sort();
        names.dedup();
        names
    }

    fn global_namespaces(&self) -> Vec<GlobalNamespace> {
        self.project.globals.global_namespaces.clone()
    }

    fn global_decls(&self, name: &str) -> Vec<DeclKey> {
        self.project
            .globals
            .global_names
            .get(name)
            .cloned()
            .unwrap_or_default()
    }

    fn collect_global_decls(&self) -> bool {
        self.project.collect_global_decls
    }

    fn global_name_unmodelled(&self, name: &str) -> bool {
        self.project.globals.unmodelled_globals.contains(name)
    }
}

// The text of a file for the checker: the text discovery already read when it kept one,
// so a file is read from the disk once. A file it did not keep (not scanned, or not valid
// UTF-8) is read here, and fails here exactly as it always did.
fn load_text<'g>(
    graph: &'g ModuleGraph,
    id: FileId,
    path: &std::path::Path,
) -> std::io::Result<Cow<'g, str>> {
    match graph.source(id) {
        Some(text) => Ok(Cow::Borrowed(text)),
        None => fs::read_to_string(path).map(Cow::Owned),
    }
}

// One unit per file. The worker is the thread's own, so nothing mutable is shared.
fn check_file(project: &Project<'_>, worker: &mut Worker, id: FileId) -> Checked {
    let graph = project.graph;
    #[cfg(test)]
    {
        use std::sync::atomic::Ordering;
        let salt = TEST_DELAY_SALT.load(Ordering::Relaxed);
        if salt != 0 {
            let delay = (id.index().wrapping_mul(17) as usize + salt * 13) % 9;
            std::thread::sleep(std::time::Duration::from_millis(delay as u64));
        }
    }
    let mut unit_exports = None;
    let mut reads = Vec::new();
    let mut incomplete_reasons = Vec::new();
    let outcome = match graph.files().path(id) {
        None => FileOutcome::ReadFailed(format!("no path recorded for file {}", id.index())),
        Some(path) => match match project.texts.and_then(|texts| texts(id)) {
            Some(text) => Ok(Cow::Owned(text.to_string())),
            None => load_text(graph, id, path),
        } {
            Err(error) => FileOutcome::ReadFailed(format!("{}: {error}", path.display())),
            Ok(source) => {
                let name = path.to_string_lossy();
                let imports = ImportsOf {
                    project,
                    from: id,
                    reads: std::sync::Mutex::new(Vec::new()),
                };
                let mut ctx = UnitContext::new(id, &name, &source)
                    .with_imports(&imports)
                    .with_options(graph.check_options());
                // A file with no stable key (inside node_modules, until package keys
                // exist) can import but exports nothing.
                if let Some(module) = graph.module_key(id) {
                    ctx = ctx.with_module(module);
                }
                if let Some(cancel) = project.cancel {
                    ctx = ctx.with_cancel(cancel.clone());
                }
                let checked = worker.check_unit(&ctx);
                reads = imports.take_reads();
                match checked {
                    Ok(output) => {
                        incomplete_reasons = output.incomplete_reasons.clone();
                        unit_exports = Some(UnitExports {
                            names: Arc::new(output.export_names),
                            decls: output.exports.into_iter().map(Arc::new).collect(),
                            complete: output.exports_complete,
                            ambient: output.ambient,
                            augmentations: output.augmentations,
                            umd_namespaces: output.umd_namespaces,
                            unresolved_names: output.unresolved_names,
                            globals: output.globals,
                            return_reads: output.return_reads,
                        });
                        FileOutcome::Checked(output.diagnostics)
                    }
                    Err(UnitError::Checker(error)) => FileOutcome::CheckFailed(error),
                    Err(UnitError::Cancelled) => FileOutcome::ReadFailed(CANCELLED.to_string()),
                }
            }
        },
    };
    (
        FileReport {
            file_id: id,
            outcome,
            reads,
            incomplete_reasons,
        },
        unit_exports,
    )
}

#[cfg(test)]
mod determinism_tests {
    use super::{ModuleGraph, TEST_DELAY_SALT, check_project};
    use crate::module_resolution::ModuleResolver;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::Ordering;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scratch_project() -> (PathBuf, Vec<PathBuf>) {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("tsox-delay-determinism-{nonce}"));
        fs::create_dir_all(&root).unwrap();
        let sources = [
            (
                "a.ts",
                "import { B } from './b'; const a: B = { value: 'bad' };",
            ),
            ("b.ts", "export interface B { value: number }"),
            ("c.ts", "export const c: number = 'bad';"),
            ("d.ts", "export const d: string = 1;"),
            ("e.ts", "export const e: boolean = false;"),
        ];
        let mut paths = Vec::new();
        for (name, source) in sources {
            let path = root.join(name);
            fs::write(&path, source).unwrap();
            paths.push(path);
        }
        (root, paths)
    }

    fn signature(report: &super::ProjectReport) -> Vec<String> {
        report
            .files
            .iter()
            .map(|file| {
                let outcome = match &file.outcome {
                    super::FileOutcome::Checked(diagnostics) => format!("checked:{diagnostics:?}"),
                    super::FileOutcome::ReadFailed(error) => format!("read:{error}"),
                    super::FileOutcome::CheckFailed(error) => format!("check:{error:?}"),
                };
                format!("{}:{:?}:{outcome}", file.file_id.index(), file.reads)
            })
            .collect()
    }

    #[test]
    fn injected_completion_delays_do_not_change_project_results() {
        let (root, entries) = scratch_project();
        let graph = ModuleGraph::build(&entries, &ModuleResolver::new()).unwrap();
        TEST_DELAY_SALT.store(1, Ordering::Relaxed);
        let first = signature(&check_project(&graph));
        TEST_DELAY_SALT.store(7, Ordering::Relaxed);
        let second = signature(&check_project(&graph));
        TEST_DELAY_SALT.store(0, Ordering::Relaxed);
        assert_eq!(
            first, second,
            "worker completion order must not change reports"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn different_fx_hash_seeds_do_not_change_project_results() {
        let (root, entries) = scratch_project();
        crate::fxhash::TEST_HASH_SEED.store(0x1357_2468, Ordering::Relaxed);
        let first_graph = ModuleGraph::build(&entries, &ModuleResolver::new()).unwrap();
        let first = signature(&check_project(&first_graph));
        crate::fxhash::TEST_HASH_SEED.store(0xdead_beef, Ordering::Relaxed);
        let second_graph = ModuleGraph::build(&entries, &ModuleResolver::new()).unwrap();
        let second = signature(&check_project(&second_graph));
        crate::fxhash::TEST_HASH_SEED.store(0, Ordering::Relaxed);
        assert_eq!(
            first, second,
            "hash bucket order must not change graph or checker results"
        );
        fs::remove_dir_all(root).unwrap();
    }

    use super::{CANCELLED, FileOutcome, PASSES_RUN, check_project_cancellable};
    use crate::scc::tests::Xorshift;
    use crate::unit::CancelToken;

    fn write_project(tag: &str, sources: &[(String, String)]) -> (PathBuf, Vec<PathBuf>) {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("tsox-{tag}-{nonce}"));
        fs::create_dir_all(&root).unwrap();
        let mut paths = Vec::new();
        for (name, source) in sources {
            let path = root.join(name);
            fs::write(&path, source).unwrap();
            paths.push(path);
        }
        (root, paths)
    }

    // Files that import lower-numbered files, with a few back edges so some form cycles, and
    // a wrong initialiser in some, so there is something to report.
    fn random_project(rng: &mut Xorshift) -> Vec<(String, String)> {
        let count = 3 + rng.below(10);
        let mut sources = Vec::new();
        for index in 0..count {
            let mut text = String::new();
            let mut used = Vec::new();
            for other in 0..count {
                let back = other > index && rng.below(12) == 0;
                let forward = other < index && rng.below(3) == 0;
                if forward || back {
                    text.push_str(&format!("import {{ v{other} }} from './f{other}';\n"));
                    used.push(other);
                }
            }
            let sum: Vec<String> = used.iter().map(|other| format!("v{other}")).collect();
            let value = if rng.below(4) == 0 { "'bad'" } else { "1" };
            text.push_str(&format!("export const v{index}: number = {value};\n"));
            if !sum.is_empty() {
                text.push_str(&format!(
                    "export const s{index}: string = {};\n",
                    sum.join(" + ")
                ));
            }
            sources.push((format!("f{index}.ts"), text));
        }
        sources
    }

    #[test]
    fn random_projects_give_the_same_report_whatever_the_timing() {
        let mut rng = Xorshift(0x9e37_79b9_7f4a_7c15);
        for round in 0..25 {
            let sources = random_project(&mut rng);
            let (root, entries) = write_project(&format!("random-{round}"), &sources);
            let graph = ModuleGraph::build(&entries, &ModuleResolver::new()).unwrap();
            let mut seen = Vec::new();
            for salt in [0, 1, 5, 11] {
                TEST_DELAY_SALT.store(salt, Ordering::Relaxed);
                seen.push(signature(&check_project(&graph)));
            }
            TEST_DELAY_SALT.store(0, Ordering::Relaxed);
            for other in &seen[1..] {
                assert_eq!(&seen[0], other, "round {round}: {sources:?}");
            }
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn a_project_with_nothing_global_is_checked_in_one_pass() {
        let (root, entries) = scratch_project();
        let graph = ModuleGraph::build(&entries, &ModuleResolver::new()).unwrap();
        check_project(&graph);
        assert_eq!(PASSES_RUN.with(|count| count.get()), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_umd_global_makes_a_second_pass() {
        let sources = vec![
            (
                "lib.d.ts".to_string(),
                "export function make(): number;\nexport as namespace Lib;".to_string(),
            ),
            (
                "use.ts".to_string(),
                "const n: number = Lib.make();".to_string(),
            ),
        ];
        let (root, entries) = write_project("umd-two-passes", &sources);
        let graph = ModuleGraph::build(&entries, &ModuleResolver::new()).unwrap();
        check_project(&graph);
        assert_eq!(PASSES_RUN.with(|count| count.get()), 2);
        fs::remove_dir_all(root).unwrap();
    }

    fn shared_global_project(tag: &str) -> (PathBuf, Vec<PathBuf>) {
        let mut sources = vec![
            (
                "decl.d.ts".to_string(),
                "interface Shared { n: number }".to_string(),
            ),
            (
                "user.ts".to_string(),
                "export const ok: Shared = { n: 1 };\nexport const bad: Shared = { n: 's' };\n"
                    .to_string(),
            ),
            (
                "importer.ts".to_string(),
                "import { ok } from './user';\nexport const t: string = ok.n;\n".to_string(),
            ),
        ];
        for index in 0..5 {
            sources.push((
                format!("lone{index}.ts"),
                format!("export const a{index}: number = {index};\n"),
            ));
        }
        write_project(tag, &sources)
    }

    #[test]
    fn a_global_declaration_rechecks_only_its_users_and_what_reads_them() {
        use super::{FILES_CHECKED, FORCE_FULL_SECOND};
        let (root, entries) = shared_global_project("narrow-second-pass");
        let graph = ModuleGraph::build(&entries, &ModuleResolver::new()).unwrap();
        let narrowed = signature(&check_project(&graph));
        assert_eq!(PASSES_RUN.with(|count| count.get()), 2);
        // Eight files in the first pass; then only `user.ts`, which uses `Shared`, and
        // `importer.ts`, which reads from it.
        assert_eq!(FILES_CHECKED.with(|count| count.get()), 8 + 2);

        FORCE_FULL_SECOND.with(|flag| flag.set(true));
        let full = signature(&check_project(&graph));
        FORCE_FULL_SECOND.with(|flag| flag.set(false));
        assert_eq!(FILES_CHECKED.with(|count| count.get()), 8 + 8);
        assert_eq!(
            narrowed, full,
            "narrowing must not change any file's result"
        );
        fs::remove_dir_all(root).unwrap();
    }

    // Checks `graph` narrowed and with the second pass forced to every file, and says how many
    // files each ran and whether the results are the same.
    fn narrowed_and_full(graph: &ModuleGraph) -> (usize, usize, bool) {
        use super::{FILES_CHECKED, FORCE_FULL_SECOND};
        let narrowed = signature(&check_project(graph));
        let narrowed_files = FILES_CHECKED.with(|count| count.get());
        assert_eq!(PASSES_RUN.with(|count| count.get()), 2);
        FORCE_FULL_SECOND.with(|flag| flag.set(true));
        let full = signature(&check_project(graph));
        FORCE_FULL_SECOND.with(|flag| flag.set(false));
        let full_files = FILES_CHECKED.with(|count| count.get());
        (narrowed_files, full_files, narrowed == full)
    }

    #[test]
    fn a_umd_global_rechecks_only_the_files_that_use_it() {
        let sources = vec![
            (
                "lib.d.ts".to_string(),
                "export function make(): number;\nexport as namespace Lib;".to_string(),
            ),
            (
                "use.ts".to_string(),
                "const n: number = Lib.make();\nconst bad: string = Lib.make();\n".to_string(),
            ),
            ("lone.ts".to_string(), "export const a = 1;\n".to_string()),
        ];
        let (root, entries) = write_project("umd-narrow", &sources);
        let graph = ModuleGraph::build(&entries, &ModuleResolver::new()).unwrap();
        let (narrowed, full, same) = narrowed_and_full(&graph);
        assert_eq!((narrowed, full), (3 + 1, 3 + 3));
        assert!(same, "narrowing must not change any file's result");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_augmentation_rechecks_the_files_that_read_the_augmented_module() {
        let mut sources = vec![
            (
                "a.ts".to_string(),
                "export interface Foo { a: number }\n".to_string(),
            ),
            (
                "aug.ts".to_string(),
                "import './a';\ndeclare module './a' { interface Foo { b: string } }\n".to_string(),
            ),
            (
                "use.ts".to_string(),
                "import { Foo } from './a';\nexport const x: Foo = { a: 1 };\n".to_string(),
            ),
        ];
        for index in 0..4 {
            sources.push((
                format!("lone{index}.ts"),
                format!("export const a{index}: number = {index};\n"),
            ));
        }
        let (root, entries) = write_project("augmentation-narrow", &sources);
        let graph = ModuleGraph::build(&entries, &ModuleResolver::new()).unwrap();
        let (narrowed, full, same) = narrowed_and_full(&graph);
        // `aug.ts` and `use.ts` import `./a`; the four others have nothing to do with it.
        assert_eq!((narrowed, full), (7 + 2, 7 + 7));
        assert!(same, "narrowing must not change any file's result");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_wildcard_ambient_module_checks_every_file_again() {
        let sources = vec![
            (
                "mixed.ts".to_string(),
                "export const m: number = 1;\n\
                 declare module '*.css' { const text: string; export default text; }\n"
                    .to_string(),
            ),
            (
                "use.ts".to_string(),
                "import { m } from './mixed';\nexport const n: string = m;\n".to_string(),
            ),
            ("lone.ts".to_string(), "export const a = 1;\n".to_string()),
        ];
        let (root, entries) = write_project("wildcard-full", &sources);
        let graph = ModuleGraph::build(&entries, &ModuleResolver::new()).unwrap();
        let (narrowed, full, same) = narrowed_and_full(&graph);
        assert_eq!(narrowed, full, "a pattern can match any specifier");
        assert!(same);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_token_cancelled_before_the_run_checks_nothing_and_says_so() {
        let (root, entries) = scratch_project();
        let graph = ModuleGraph::build(&entries, &ModuleResolver::new()).unwrap();
        let token = CancelToken::new();
        token.cancel();
        let report = check_project_cancellable(&graph, &token);
        assert!(report.cancelled);
        assert_eq!(report.files.len(), entries.len());
        for file in &report.files {
            assert!(
                matches!(&file.outcome, FileOutcome::ReadFailed(text) if text == CANCELLED),
                "{:?}",
                file.outcome
            );
        }
        assert_eq!(report.failure_count(), entries.len());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_token_that_is_never_cancelled_changes_nothing() {
        let (root, entries) = scratch_project();
        let graph = ModuleGraph::build(&entries, &ModuleResolver::new()).unwrap();
        let plain = signature(&check_project(&graph));
        let token = CancelToken::new();
        let report = check_project_cancellable(&graph, &token);
        assert!(!report.cancelled);
        assert_eq!(plain, signature(&report));
        fs::remove_dir_all(root).unwrap();
    }
}
