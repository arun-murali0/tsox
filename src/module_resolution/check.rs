use std::fs;
use std::sync::{Arc, OnceLock};

use rayon::prelude::*;

use crate::bridge::IncompleteReason;
use crate::diagnostics::Diagnostic;
use crate::error::CheckerError;
use crate::fxhash::FxHashMap;
use crate::keys::DeclKey;
use crate::portable::{DeclProvider, GlobalNamespace, ImportSource, ModuleExports, PortableDecl};
use crate::types::FileId;
use crate::unit::{UnitContext, UnitError, Worker};

use super::graph::ModuleGraph;

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

/// Checks every checkable file of `graph`, one layer at a time, the files of a layer in
/// parallel. The report is sorted by file id, so it does not depend on which thread
/// finished first.
///
/// An import is looked up in the file it names: a file is checked only after everything
/// it imports has been, and what those files export (their annotated interfaces, aliases,
/// functions and constants, as portable declarations) is what its imports bind to. Files
/// of one layer are checked against the same snapshot, taken before the layer starts, so
/// the result does not depend on how the threads interleave. The members of a cycle share
/// a layer, so their first pass finds nothing among themselves; they are then checked
/// again against what that pass exported, until what they export stops changing, or until
/// a bound that grows with the size of the cycle runs out. A cycle that hits the bound is
/// listed in `ProjectReport::unsettled` and its members export as incomplete. An import of
/// a file that is not checked (a dependency's declaration file) binds nothing, which
/// leaves the name unknown: it loses an error and never invents one.
pub fn check_project(graph: &ModuleGraph) -> ProjectReport {
    check_project_bounded(graph, None)
}

/// `check_project` with the recheck bound of every cycle replaced by `max_cycle_rounds`
/// (`None` keeps the default, see `cycle_round_cap`). Exists so a test can force a cycle
/// to run out of rounds, which no ordinary project does.
#[doc(hidden)]
pub fn check_project_bounded(
    graph: &ModuleGraph,
    max_cycle_rounds: Option<usize>,
) -> ProjectReport {
    let mut files = Vec::new();
    let mut unsettled: Vec<FileId> = Vec::new();
    let mut exports = ProjectExports::default();
    let in_cycle: std::collections::HashSet<FileId> =
        graph.cycles().into_iter().flatten().collect();
    // A file that only declares ambient modules is imported by nobody, so no edge puts it
    // ahead of the files that use what it declares. Those files are checked first.
    let providers = ambient_providers(graph);
    if !providers.is_empty() {
        for (report, unit_exports) in check_files(graph, &exports, &providers) {
            if let Some(unit_exports) = unit_exports {
                exports.add(graph, report.file_id, unit_exports);
            }
            if !graph.is_dependency_declaration(report.file_id) {
                files.push(report);
            }
        }
    }
    exports.bare_reportable = bare_specifiers_are_reportable(graph, &exports);
    for layer in graph.layers() {
        let layer: Vec<FileId> = layer
            .into_iter()
            .filter(|id| !providers.contains(id))
            .collect();
        let mut done = check_files(graph, &exports, &layer);
        let members: Vec<FileId> = layer
            .iter()
            .copied()
            .filter(|id| in_cycle.contains(id))
            .collect();
        if !members.is_empty() {
            settle_members(
                graph,
                &mut exports,
                &members,
                &mut done,
                max_cycle_rounds,
                &mut unsettled,
            );
        }
        for (report, unit_exports) in done {
            if let Some(unit_exports) = unit_exports {
                exports.add(graph, report.file_id, unit_exports);
            }
            // A package's declaration file is read for what it exports and never reported.
            if !graph.is_dependency_declaration(report.file_id) {
                files.push(report);
            }
        }
    }

    // `export as namespace` exposes a module without a static import edge from its
    // consumers. The ordinary dependency-first pass therefore cannot know every global
    // before checking each script. Once all module export tables are available, recheck
    // the non-provider files against the complete UMD map. This path only runs for
    // projects that actually declare UMD globals.
    if !exports.umd_by_file.is_empty() || !exports.augmentations.is_empty() {
        let provider_ids: std::collections::HashSet<FileId> = providers.iter().copied().collect();
        let mut final_files: Vec<FileReport> = files
            .into_iter()
            .filter(|report| provider_ids.contains(&report.file_id))
            .collect();

        for layer in graph.layers() {
            let layer: Vec<FileId> = layer
                .into_iter()
                .filter(|id| !provider_ids.contains(id))
                .collect();
            let mut done = check_files(graph, &exports, &layer);
            let members: Vec<FileId> = layer
                .iter()
                .copied()
                .filter(|id| in_cycle.contains(id))
                .collect();
            if !members.is_empty() {
                settle_members(
                    graph,
                    &mut exports,
                    &members,
                    &mut done,
                    max_cycle_rounds,
                    &mut unsettled,
                );
            }
            for (report, unit_exports) in done {
                if let Some(unit_exports) = unit_exports {
                    exports.add(graph, report.file_id, unit_exports);
                }
                if !graph.is_dependency_declaration(report.file_id) {
                    final_files.push(report);
                }
            }
        }
        files = final_files;
    }
    files.sort_by_key(|report| report.file_id);
    unsettled.sort();
    unsettled.dedup();
    ProjectReport { files, unsettled }
}

// Whether a package specifier that no file answers (`import x from \"react\"`) can be called
// a missing module. TypeScript also finds one through `paths`, `baseUrl`, `rootDirs` or an
// `extends`ed config (none of which are read here), through a `declare module` pattern
// (`\"*.css\"`), and through any `@types` package, whose declarations are global and are
// not files this project imports. A project with none of those has nothing else to
// answer it, so the specifier really is missing; with any of them it is left alone.
fn bare_specifiers_are_reportable(graph: &ModuleGraph, exports: &ProjectExports) -> bool {
    let Some(root) = graph.root() else {
        return false;
    };
    if exports
        .ambient
        .keys()
        .any(|specifier| specifier.contains('*'))
    {
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
// modules. Parse the file to distinguish module-level imports/exports from declarations
// inside a `declare module` block. These providers are checked before consumers so their
// portable exports are available even though they do not have ordinary import edges.
fn ambient_providers(graph: &ModuleGraph) -> Vec<FileId> {
    use oxc_allocator::Allocator;
    use oxc_ast::ast::Statement;
    use oxc_parser::Parser;
    use oxc_span::SourceType;

    let mut found = Vec::new();
    for id in graph.layers().into_iter().flatten() {
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
        if !is_declaration {
            continue;
        }
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        // Do not infer module status with a line prefix or a substring. Comments,
        // whitespace, and quote style make those checks incorrect; the parser already
        // provides the syntax-level distinction used by the resolver.
        let allocator = Allocator::default();
        let source_type = SourceType::from_path(path)
            .unwrap_or_else(|_| SourceType::default().with_typescript(true));
        let parsed = Parser::new(&allocator, &text, source_type).parse();
        if parsed.fatal_error {
            continue;
        }
        let declares = parsed
            .program
            .body
            .iter()
            .any(|statement| matches!(statement, Statement::TSExternalModuleDeclaration(_)));
        let is_module = parsed.program.body.iter().any(|statement| {
            matches!(
                statement,
                Statement::ImportDeclaration(_)
                    | Statement::ExportDeclaration(_)
                    | Statement::ExportNamedDeclaration(_)
                    | Statement::ExportDefaultDeclaration(_)
                    | Statement::ExportAllDeclaration(_)
                    | Statement::ExportFromDeclaration(_)
                    | Statement::TSExportAssignment(_)
                    | Statement::TSNamespaceExportDeclaration(_)
                    | Statement::TSImportEqualsDeclaration(_)
            )
        });
        if declares && !is_module {
            found.push(id);
        }
    }
    found
}

// Small cycles keep the old low recheck cost. Longer cycles need more rounds because an
// export can travel only one re-export edge per snapshot: a value declared near one end
// of a ring may take O(cycle length) passes to reach the other end. The extra pass lets
// us observe that the exports have stopped changing. This remains a safety bound, not a
// proof of convergence for arbitrary inferred-signature cycles.
const MIN_CYCLE_ROUNDS: usize = 4;

// Settles one layer's cycle members and records the ones that did not settle. When the
// bound runs out while their exports are still changing, what they export may be missing
// names, so it is passed on as a partial list: an importer then stays quiet about a name it
// cannot find instead of reporting it.
fn settle_members(
    graph: &ModuleGraph,
    exports: &mut ProjectExports,
    members: &[FileId],
    done: &mut [(FileReport, Option<UnitExports>)],
    max_cycle_rounds: Option<usize>,
    unsettled: &mut Vec<FileId>,
) {
    let rounds = max_cycle_rounds.unwrap_or_else(|| cycle_round_cap(members.len()));
    if settle_cycle(graph, exports, members, done, rounds) {
        return;
    }
    for (report, unit_exports) in done.iter_mut() {
        if members.contains(&report.file_id)
            && let Some(unit_exports) = unit_exports
        {
            unit_exports.complete = false;
        }
    }
    unsettled.extend(members.iter().copied());
}

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

// Checks the files of one layer in parallel, against `exports` as it stands.
fn check_files(
    graph: &ModuleGraph,
    exports: &ProjectExports,
    ids: &[FileId],
) -> Vec<(FileReport, Option<UnitExports>)> {
    let check_layer = || {
        ids.par_iter()
            .filter(|id| graph.is_checkable(**id) || graph.is_dependency_declaration(**id))
            // One worker per thread, so its arena is reused from file to file; the
            // worker clears it after every unit, so nothing carries over.
            .map_init(Worker::new, |worker, id| {
                #[cfg(test)]
                {
                    use std::sync::atomic::Ordering;
                    let salt = TEST_DELAY_SALT.load(Ordering::Relaxed);
                    if salt != 0 {
                        let delay = (id.index().wrapping_mul(17) as usize + salt * 13) % 9;
                        std::thread::sleep(std::time::Duration::from_millis(delay as u64));
                    }
                }
                check_file(graph, exports, worker, *id)
            })
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
    graph: &ModuleGraph,
    exports: &mut ProjectExports,
    members: &[FileId],
    done: &mut [(FileReport, Option<UnitExports>)],
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
                    exports.add(graph, report.file_id, unit_exports.clone());
                }
            }
        }
        let again = check_files(graph, exports, members);
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

// Everything the checked files export so far: which names each module exports, and the
// declarations by key. Keys carry their module, so one map holds every module's. Only ever
// looked up, never walked, so hash order cannot reach a result (LLD 8.2).
#[derive(Default)]
struct ProjectExports {
    names: FxHashMap<FileId, Vec<(String, DeclKey)>>,
    complete: FxHashMap<FileId, bool>,
    decls: FxHashMap<DeclKey, Arc<PortableDecl>>,
    // The modules some file declared with `declare module "x"`, by specifier: their export
    // names, and whether those are the whole list. A specifier several blocks declare is
    // their union.
    ambient: FxHashMap<String, (Vec<(String, DeclKey)>, bool)>,
    // Module augmentation declarations are merged with a resolved module's export list;
    // unlike ambient modules, they must not make an otherwise unresolved specifier exist.
    augmentations: FxHashMap<FileId, (Vec<(String, DeclKey)>, bool)>,
    // UMD aliases are attached to the source file, and their members are derived from its
    // current export table. Replacing a file's outputs therefore also replaces its global
    // shape instead of leaving stale keys behind after an edit.
    umd_by_file: FxHashMap<FileId, Vec<String>>,
    // Whether a package specifier no file answers is certainly a missing module (see
    // `bare_specifiers_are_reportable`).
    bare_reportable: bool,
}

impl ProjectExports {
    fn add(&mut self, graph: &ModuleGraph, id: FileId, unit: UnitExports) {
        self.names.insert(id, unit.names);
        self.complete.insert(id, unit.complete);
        self.umd_by_file.insert(id, unit.umd_namespaces);
        for decl in unit.decls {
            self.decls.insert(decl.key, Arc::new(decl));
        }
        for module in unit.ambient {
            for decl in module.exports {
                self.decls.insert(decl.key, Arc::new(decl));
            }
            let entry = self
                .ambient
                .entry(module.specifier)
                .or_insert_with(|| (Vec::new(), true));
            entry.0.extend(module.export_names);
            entry.0.sort();
            entry.0.dedup();
            entry.1 &= module.complete;
        }
        for module in unit.augmentations {
            // Augmentations attach to the resolved module, not to the text of the
            // specifier. Different consumers may name the same file as `./x`, `../p/x`,
            // or through a package export. The graph's edge is the authoritative identity.
            let target = graph
                .edges(id)
                .iter()
                .find(|edge| {
                    edge.kind == super::discovery::ModuleRequestKind::ModuleAugmentation
                        && edge.specifier == module.specifier
                })
                .and_then(|edge| edge.target);
            for decl in module.exports {
                self.decls.insert(decl.key, Arc::new(decl));
            }
            let Some(target) = target else {
                continue;
            };
            let entry = self
                .augmentations
                .entry(target)
                .or_insert_with(|| (Vec::new(), true));
            entry.0.extend(module.export_names);
            entry.0.sort();
            entry.0.dedup();
            entry.1 &= module.complete;
        }
    }

    fn global_namespaces(&self) -> Vec<GlobalNamespace> {
        let mut namespaces: std::collections::BTreeMap<
            String,
            (Vec<(String, DeclKey)>, Option<(FileId, DeclKey)>),
        > = std::collections::BTreeMap::new();
        let mut files: Vec<FileId> = self.umd_by_file.keys().copied().collect();
        files.sort();
        for file_id in files {
            let Some(aliases) = self.umd_by_file.get(&file_id) else {
                continue;
            };
            let Some(names) = self.names.get(&file_id) else {
                continue;
            };
            let assignment = names
                .iter()
                .find(|(name, _)| name == "export=")
                .map(|(_, key)| *key);
            for alias in aliases {
                let (members, current_assignment) = namespaces.entry(alias.clone()).or_default();
                members.extend(
                    names
                        .iter()
                        .filter(|(name, _)| name != "default" && name != "export=")
                        .cloned(),
                );
                if let Some(key) = assignment {
                    if current_assignment.is_none_or(|(current_file, _)| file_id < current_file) {
                        *current_assignment = Some((file_id, key));
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
}

// The imports of one file: its specifiers are resolved through the graph's own edges, so
// the resolution the graph already did (and cached) is the one used here.
struct ImportsOf<'a> {
    graph: &'a ModuleGraph,
    exports: &'a ProjectExports,
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
        self.graph
            .edges(self.from)
            .iter()
            .find(|edge| edge.specifier == specifier)
            .and_then(|edge| edge.target)
    }

    fn append_augmentations(&self, target: FileId, names: &mut Vec<(String, DeclKey)>) {
        if let Some((extra, _)) = self.exports.augmentations.get(&target) {
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
        self.exports.decls.get(&key).cloned()
    }
}

impl ImportSource for ImportsOf<'_> {
    fn export_all(&self, specifier: &str) -> Vec<(String, DeclKey)> {
        // A `declare module "x"` answers `x` before any file does, as it does in TypeScript.
        if let Some((names, _)) = self.exports.ambient.get(specifier) {
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
            .exports
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
        if let Some((_, complete)) = self.exports.ambient.get(specifier) {
            return ModuleExports::Known {
                complete: *complete,
            };
        }
        let Some(edge) = self
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
            return if relative || (self.exports.bare_reportable && !is_special_specifier(specifier))
            {
                ModuleExports::Unresolved
            } else {
                ModuleExports::Unknown
            };
        };
        match self.exports.complete.get(&target) {
            Some(complete) => ModuleExports::Known {
                complete: *complete
                    && self
                        .exports
                        .augmentations
                        .get(&target)
                        .is_none_or(|(_, augmentation_complete)| *augmentation_complete),
            },
            None => ModuleExports::Unknown,
        }
    }

    fn export_keys(&self, specifier: &str, name: &str) -> Vec<DeclKey> {
        if let Some((names, _)) = self.exports.ambient.get(specifier) {
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
            .exports
            .names
            .get(&target)
            .into_iter()
            .flatten()
            .filter(|(exported, _)| exported == name)
            .map(|(_, key)| *key)
            .collect::<Vec<_>>();
        if let Some((extra, _)) = self.exports.augmentations.get(&target) {
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
        self.exports.global_namespaces()
    }
}

// One unit per file. The worker is the thread's own, so nothing mutable is shared.
fn check_file(
    graph: &ModuleGraph,
    exports: &ProjectExports,
    worker: &mut Worker,
    id: FileId,
) -> (FileReport, Option<UnitExports>) {
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
                    graph,
                    exports,
                    from: id,
                    reads: std::sync::Mutex::new(Vec::new()),
                };
                let mut ctx = UnitContext::new(id, &name, &source).with_imports(&imports);
                // A file with no stable key (inside node_modules, until package keys
                // exist) can import but exports nothing.
                if let Some(module) = graph.module_key(id) {
                    ctx = ctx.with_module(module);
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
                    // No token is given to these units, so nothing cancels them.
                    Err(UnitError::Cancelled) => {
                        FileOutcome::ReadFailed("the check was cancelled".to_string())
                    }
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
        let root = std::env::temp_dir().join(format!("ts-rust-delay-determinism-{nonce}"));
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
}
