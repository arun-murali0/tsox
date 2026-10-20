use std::fs;
use std::sync::{Arc, OnceLock, RwLock};

use rayon::prelude::*;

use crate::bridge::IncompleteReason;
use crate::diagnostics::Diagnostic;
use crate::error::CheckerError;
use crate::fxhash::FxHashMap;
use crate::keys::DeclKey;
use crate::portable::{DeclProvider, GlobalNamespace, ImportSource, ModuleExports, PortableDecl};
use crate::types::FileId;
use crate::unit::{CancelToken, UnitContext, UnitError, Worker};

use super::graph::ModuleGraph;
use super::schedule::{UnitOutcome, run_units, unit_dependencies};

// What happened to one file. A file that could not be read or parsed is reported as
// such and is never left out, because a report that silently omits files reads as "all
// clear" for exactly the files nobody looked at.
#[derive(Debug)]
pub enum FileOutcome {
    Checked(Vec<Diagnostic>),
    ReadFailed(String),
    CheckFailed(CheckerError),
}

#[derive(Debug)]
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
}

impl Project<'_> {
    fn publish(&self, id: FileId, unit: UnitExports) {
        self.tables
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .add(id, unit);
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
    names: FxHashMap<FileId, Vec<(String, DeclKey)>>,
    complete: FxHashMap<FileId, bool>,
    decls: FxHashMap<DeclKey, Arc<PortableDecl>>,
}

impl Tables {
    fn add(&mut self, id: FileId, unit: UnitExports) {
        self.names.insert(id, unit.names);
        self.complete.insert(id, unit.complete);
        for decl in unit.decls {
            self.decls.insert(decl.key, Arc::new(decl));
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
        // A script reaches the members of a UMD global without an import edge, so it does
        // not wait for the file that exports it. The declarations of that file, and of
        // everything it imports, are therefore handed over here, fixed for the pass, and
        // not read from the tables of whichever file happens to have finished.
        let by_file: FxHashMap<FileId, &UnitExports> = units.iter().copied().collect();
        let mut seen: std::collections::HashSet<FileId> = std::collections::HashSet::new();
        let mut pending: Vec<FileId> = units
            .iter()
            .filter(|(_, unit)| !unit.umd_namespaces.is_empty())
            .map(|(id, _)| *id)
            .collect();
        while let Some(id) = pending.pop() {
            if !seen.insert(id) {
                continue;
            }
            if let Some(unit) = by_file.get(&id) {
                for decl in &unit.decls {
                    globals.decls.insert(decl.key, Arc::new(decl.clone()));
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
type Checked = (FileReport, Option<UnitExports>);

// What one unit hands back: its files, and the files of a cycle that did not settle.
#[derive(Default)]
struct UnitResult {
    files: Vec<Checked>,
    unsettled: Vec<FileId>,
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
    run_project(graph, None, None)
}

/// `check_project` with the recheck bound of every cycle replaced by `max_cycle_rounds`
/// (`None` keeps the default, see `cycle_round_cap`). Exists so a test can force a cycle
/// to run out of rounds, which no ordinary project does.
#[doc(hidden)]
pub fn check_project_bounded(
    graph: &ModuleGraph,
    max_cycle_rounds: Option<usize>,
) -> ProjectReport {
    run_project(graph, max_cycle_rounds, None)
}

/// `check_project` that stops when `cancel` is cancelled. The token is looked at as each
/// unit starts and inside each file's check; a file that did not run is in the report as a
/// read failure that says so, and `ProjectReport::cancelled` is set, so a cancelled run is
/// never mistaken for a clean one. Files that did run keep their results.
pub fn check_project_cancellable(graph: &ModuleGraph, cancel: &CancelToken) -> ProjectReport {
    run_project(graph, None, Some(cancel))
}

fn run_project(
    graph: &ModuleGraph,
    max_cycle_rounds: Option<usize>,
    cancel: Option<&CancelToken>,
) -> ProjectReport {
    let in_cycle: std::collections::HashSet<FileId> =
        graph.cycles().into_iter().flatten().collect();
    // A file that only declares ambient modules is imported by nobody, so no edge puts it
    // ahead of the files that use what it declares. Those files are checked first.
    let providers = ambient_providers(graph);
    let mut files: Vec<FileReport> = Vec::new();
    let mut unsettled: Vec<FileId> = Vec::new();
    let mut provider_units: Vec<(FileId, UnitExports)> = Vec::new();
    if !providers.is_empty() {
        let project = Project {
            graph,
            tables: RwLock::new(Tables::default()),
            globals: Globals::default(),
            cancel,
        };
        for (report, unit_exports) in check_files(&project, &providers) {
            if let Some(unit_exports) = unit_exports {
                provider_units.push((report.file_id, unit_exports));
            }
            if !graph.is_dependency_declaration(report.file_id) {
                files.push(report);
            }
        }
        provider_units.sort_by_key(|(id, _)| *id);
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
    };

    #[cfg(test)]
    PASSES_RUN.with(|count| count.set(0));
    let first = run_pass(&setup, globals);
    unsettled.extend(first.unsettled.iter().copied());

    // `export as namespace` exposes a module without a static import edge from its
    // consumers, and a module augmentation or an ambient module in a file that is not a
    // provider does not order the files that see it either. So the first pass cannot know
    // every global before checking each file. When the first pass found any, check the
    // non-provider files again against all of them. A project with none has nothing a
    // second pass could change, so the first pass is the answer.
    let needs_second = first.files.iter().any(|(_, unit)| {
        unit.as_ref().is_some_and(|unit| {
            !unit.umd_namespaces.is_empty()
                || !unit.augmentations.is_empty()
                || !unit.ambient.is_empty()
        })
    }) && !cancel.is_some_and(CancelToken::is_cancelled);
    let last = if needs_second {
        let mut all: Vec<(FileId, &UnitExports)> = provider_refs.clone();
        all.extend(
            first
                .files
                .iter()
                .filter_map(|(report, unit)| unit.as_ref().map(|u| (report.file_id, u))),
        );
        all.sort_by_key(|(id, _)| *id);
        let mut second_globals = Globals::build(graph, &all);
        second_globals.bare_reportable = bare_specifiers_are_reportable(graph, wildcard_declared);
        let second = run_pass(&setup, second_globals);
        unsettled.extend(second.unsettled.iter().copied());
        second
    } else {
        first
    };
    for (report, _) in last.files {
        // A package's declaration file is read for what it exports and never reported.
        if !graph.is_dependency_declaration(report.file_id) {
            files.push(report);
        }
    }
    files.sort_by_key(|report| report.file_id);
    unsettled.sort();
    unsettled.dedup();
    let cancelled = files.iter().any(
        |report| matches!(&report.outcome, FileOutcome::ReadFailed(text) if text == CANCELLED),
    );
    ProjectReport {
        files,
        unsettled,
        cancelled,
    }
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
// a missing module. TypeScript also finds one through `paths`, `baseUrl`, `rootDirs` or an
// `extends`ed config (none of which are read here), through a `declare module` pattern
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
    for directory in root.ancestors() {
        if directory.join("node_modules").join("@types").is_dir() {
            return false;
        }
        for name in ["tsconfig.json", "jsconfig.json"] {
            if let Ok(text) = fs::read_to_string(directory.join(name)) {
                if [
                    "\"paths\"",
                    "\"baseUrl\"",
                    "\"rootDirs\"",
                    "\"extends\"",
                    "\"typeRoots\"",
                ]
                .iter()
                .any(|key| text.contains(key))
                {
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
    Vec<(String, DeclKey)>,
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
#[derive(Clone)]
struct UnitExports {
    names: Vec<(String, DeclKey)>,
    decls: Vec<PortableDecl>,
    // Whether `names` is the module's whole export list (see UnitOutput).
    complete: bool,
    // The modules the file declares with `declare module "x"`.
    ambient: Vec<crate::unit::AmbientExports>,
    augmentations: Vec<crate::unit::AmbientExports>,
    umd_namespaces: Vec<String>,
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

    fn target(&self, specifier: &str) -> Option<FileId> {
        self.project
            .graph
            .edges(self.from)
            .iter()
            .find(|edge| edge.specifier == specifier)
            .and_then(|edge| edge.target)
    }

    fn append_augmentations(&self, target: FileId, names: &mut Vec<(String, DeclKey)>) {
        if let Some((extra, _)) = self.project.globals.augmentations.get(&target) {
            names.extend(extra.iter().cloned());
        }
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
            .flatten()
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
        let Some(edge) = self
            .project
            .graph
            .edges(self.from)
            .iter()
            .find(|edge| edge.specifier == specifier)
        else {
            return ModuleExports::Unknown;
        };
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
            .flatten()
            .filter(|(exported, _)| exported == name)
            .map(|(_, key)| *key)
            .collect::<Vec<_>>();
        if let Some((extra, _)) = self.project.globals.augmentations.get(&target) {
            names.extend(
                extra
                    .iter()
                    .filter(|(exported, _)| exported == name)
                    .map(|(_, key)| *key),
            );
        }
        names.sort();
        names.dedup();
        names
    }

    fn global_namespaces(&self) -> Vec<GlobalNamespace> {
        self.project.globals.global_namespaces.clone()
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
        Some(path) => match fs::read_to_string(path) {
            Err(error) => FileOutcome::ReadFailed(format!("{}: {error}", path.display())),
            Ok(source) => {
                let name = path.to_string_lossy();
                let imports = ImportsOf {
                    project,
                    from: id,
                    reads: std::sync::Mutex::new(Vec::new()),
                };
                let mut ctx = UnitContext::new(id, &name, &source).with_imports(&imports);
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
                            names: output.export_names,
                            decls: output.exports,
                            complete: output.exports_complete,
                            ambient: output.ambient,
                            augmentations: output.augmentations,
                            umd_namespaces: output.umd_namespaces,
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
