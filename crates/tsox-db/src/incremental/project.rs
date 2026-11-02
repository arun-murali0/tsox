//! The query layer over the project driver (E7, LLD 6.2 to 6.7).
//!
//! Two planes. Salsa decides what must be recomputed; the checker (`check_unit_isolated`,
//! the same code the full driver runs for one unit) does the work and knows nothing about
//! Salsa. The scheduler of `check_project` stays in charge of order, and asks this layer
//! for each unit: a unit whose inputs did not change is answered from its memo.
//!
//! Inputs: one `SourceFile` per file (text and hash) and one `ProjectInput` (the module
//! graph and its epoch). Queries:
//!
//! - `module_header(file)`: what the graph reads from a file, without positions, so an edit
//!   that moves text but changes no import, export name or flag is cut off here.
//! - `check_scc(project, scc)`: one component of the graph, checked. Reads the text of its
//!   own members, and reads what its dependencies exported by projections below.
//! - `decl_shape(project, decl)` and `export_names(project, file)`: one declaration, or one
//!   module's export list, out of a unit's output. Equal to the previous value means every
//!   unit that read only that stays untouched (declaration-level cutoff).
//! - `file_report(project, file)`: one file's report, for tooling that wants to know which
//!   files changed.
//!
//! The graph is rebuilt, and the epoch moves, when a file is added or removed, an import
//! or other header content changes, or a config file changes. A rebuilt graph renumbers
//! file ids, which every output carries, so everything is checked again then; a body or
//! signature edit leaves the graph alone and reruns only what it reaches.
//!
//! A unit reads its dependencies' exports from the store (`Bundle::outputs`), not through
//! a query, because a query's result would make the unit depend on all of it. The reads the
//! check made are then registered as projection queries, so the dependency is exactly the
//! declarations and export lists it touched. That is only sound when each dependency is up
//! to date before the unit runs, which the scheduler guarantees (a unit waits for the units
//! it imports). `Bundle::verified` records it, and a unit that finds a dependency not
//! verified in this generation calls the dependency's query itself.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};

use salsa::Setter;

use super::{Database, IncrementalDb, SourceFile, SourceKind};
use crate::keys::DeclKey;
use crate::module_resolution::check::{
    Checked, RunOptions, UnitExports, UnitMemo, UnitResult, check_provider_isolated,
    check_unit_isolated, provider_ids, run_project, unit_environment,
};
use crate::module_resolution::{
    FileState, ModuleGraph, ModuleRequestKind, ModuleResolver, ProjectConfig, ProjectReport,
    scan_module_requests, unit_dependencies,
};
use crate::portable::PortableDecl;
use crate::types::FileId;

// ---------------------------------------------------------------------------------------
// Values the queries return. Each is compared by a digest of what a reader can see, so a
// recomputation that changes nothing visible is cut off.

/// What the graph reads from a file. No offsets: moving text is not a change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleHeader {
    requests: Vec<(String, ModuleRequestKind, bool, bool)>,
    parse_failed: bool,
    declares_ambient_modules: bool,
}

/// One unit's checked output. Equal when its reports and everything it hands on are.
pub struct SccOutput {
    result: UnitResult,
    // Shared copies of the exports, for the units that read them.
    shared: Vec<(FileId, Arc<UnitExports>)>,
    // 128 bits: equality of two outputs is decided by this alone, and a collision would keep
    // a stale result, so it is not left at the width a hash map would be content with.
    digest: u128,
}

impl PartialEq for SccOutput {
    fn eq(&self, other: &Self) -> bool {
        self.digest == other.digest
    }
}
impl Eq for SccOutput {}

impl SccOutput {
    fn new(result: UnitResult) -> Self {
        let mut text = String::new();
        for (report, exports) in &result.files {
            text.push_str(&format!("{report:?}"));
            if let Some(exports) = exports {
                text.push_str(&export_signature(exports));
            }
        }
        text.push_str(&format!("{:?}", result.unsettled));
        let digest = xxhash_rust::xxh3::xxh3_128(text.as_bytes());
        let shared = result
            .files
            .iter()
            .filter_map(|(report, exports)| {
                exports
                    .as_ref()
                    .map(|exports| (report.file_id, Arc::new(exports.clone())))
            })
            .collect();
        Self {
            result,
            shared,
            digest,
        }
    }
}

// Everything about an export set a reader could depend on, with declarations by signature.
fn export_signature(exports: &UnitExports) -> String {
    let sigs: Vec<_> = exports
        .decls
        .iter()
        .map(|decl| (decl.key, decl.sig))
        .collect();
    format!(
        "{:?}|{}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}",
        exports.names,
        exports.complete,
        sigs,
        exports.ambient,
        exports.augmentations,
        exports.umd_namespaces,
        exports.globals,
        exports.return_reads,
    ) + &format!("{:?}", exports.unresolved_names)
}

/// One declaration, equal when its key and signature are.
#[derive(Clone)]
pub struct DeclShape(pub Option<Arc<PortableDecl>>);

impl PartialEq for DeclShape {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (None, None) => true,
            (Some(a), Some(b)) => a.key == b.key && a.sig == b.sig,
            _ => false,
        }
    }
}
impl Eq for DeclShape {}

/// One module's export list.
#[derive(Clone, PartialEq, Eq)]
pub struct NamesEntry {
    names: Vec<(String, DeclKey)>,
    complete: bool,
}

/// One file's report, equal when what it shows is.
#[derive(Clone)]
pub struct FileReportView {
    pub report: crate::FileReport,
    digest: u64,
}

impl PartialEq for FileReportView {
    fn eq(&self, other: &Self) -> bool {
        self.digest == other.digest
    }
}
impl Eq for FileReportView {}

// ---------------------------------------------------------------------------------------
// The graph and what is derived from it once per epoch.

struct SccInfo {
    key: u64,
    // Every unit this one may read from, directly or through declarations it names.
    dependencies: Vec<usize>,
    // The units of files this one imports directly.
    direct: Vec<usize>,
}

/// The module graph of one epoch with the lookup tables the queries need. Replaced as a
/// whole when the epoch moves.
pub struct Bundle {
    graph: ModuleGraph,
    in_cycle: HashSet<FileId>,
    sources: Vec<SourceFile>,
    // Files whose text is not valid UTF-8: the checker reports them unreadable, as it does
    // when it reads them itself.
    unreadable: Vec<bool>,
    sccs: Vec<SccInfo>,
    scc_by_key: HashMap<u64, usize>,
    file_scc: Vec<usize>,
    // A file's identity across epochs: a hash of its path. A file id changes when the graph is
    // rebuilt, this does not.
    file_key: Vec<u64>,
    file_by_key: HashMap<u64, u32>,
    // What every unit reads from the configuration, as text; part of each unit's plan.
    environment: String,
    // The latest output of each unit, for the units that read from it.
    outputs: Vec<RwLock<Weak<SccOutput>>>,
    // The unit a declaration is exported from, filled as units are checked.
    owners: RwLock<HashMap<DeclKey, usize>>,
    // The generation of `check` in which each unit was last brought up to date.
    verified: Vec<AtomicU64>,
    generation: AtomicU64,
}

impl std::fmt::Debug for Bundle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Bundle({} files)", self.sources.len())
    }
}

impl Bundle {
    fn new(graph: ModuleGraph, sources: Vec<SourceFile>, unreadable: Vec<bool>) -> Self {
        let in_cycle: HashSet<FileId> = graph.cycles().into_iter().flatten().collect();
        let waits = unit_dependencies(&graph);
        let count = graph.sccs().len();
        // The transitive closure, in unit order: a unit's waits all have lower closures
        // already, because the units are numbered so that a unit comes after what it waits for.
        // They are not guaranteed to be, so the closure is a search, not a fold.
        let mut sccs = Vec::with_capacity(count);
        for unit in 0..count {
            let mut seen: HashSet<usize> = HashSet::new();
            let mut stack: Vec<usize> = waits[unit].clone();
            while let Some(next) = stack.pop() {
                if next != unit && seen.insert(next) {
                    stack.extend(waits[next].iter().copied());
                }
            }
            let mut dependencies: Vec<usize> = seen.into_iter().collect();
            dependencies.sort_unstable();
            let mut direct: Vec<usize> =
                waits[unit].iter().copied().filter(|d| *d != unit).collect();
            direct.sort_unstable();
            direct.dedup();
            let mut paths: Vec<String> = graph.sccs()[unit]
                .iter()
                .filter_map(|id| graph.files().path(*id))
                .map(|path| path.to_string_lossy().into_owned())
                .collect();
            paths.sort();
            sccs.push(SccInfo {
                key: xxhash_rust::xxh3::xxh3_64(paths.join("\u{0}").as_bytes()),
                dependencies,
                direct,
            });
        }
        let mut scc_by_key = HashMap::new();
        for (index, info) in sccs.iter_mut().enumerate() {
            // A collision of two path hashes would make two units one query; keep them apart.
            while scc_by_key.contains_key(&info.key) {
                info.key = info.key.wrapping_add(1);
            }
            scc_by_key.insert(info.key, index);
        }
        let mut file_scc = vec![0usize; graph.len()];
        for (unit, members) in graph.sccs().iter().enumerate() {
            for member in members {
                file_scc[member.index() as usize] = unit;
            }
        }
        let file_key: Vec<u64> = (0..graph.len())
            .map(|index| path_key(graph.files().path(FileId::new(index as u32))))
            .collect();
        let file_by_key = file_key
            .iter()
            .enumerate()
            .map(|(index, key)| (*key, index as u32))
            .collect();
        let environment = unit_environment(&graph);
        Self {
            file_key,
            file_by_key,
            environment,
            outputs: (0..count).map(|_| RwLock::new(Weak::new())).collect(),
            verified: (0..count).map(|_| AtomicU64::new(0)).collect(),
            generation: AtomicU64::new(0),
            owners: RwLock::new(HashMap::new()),
            graph,
            in_cycle,
            sources,
            unreadable,
            sccs,
            scc_by_key,
            file_scc,
        }
    }

    fn text(&self, db: &dyn IncrementalDb, id: FileId) -> Option<Arc<str>> {
        let index = id.index() as usize;
        if *self.unreadable.get(index)? {
            return None;
        }
        Some(Arc::clone(self.sources.get(index)?.text(db)))
    }
}

// ---------------------------------------------------------------------------------------
// Inputs and queries.

#[salsa::input]
pub struct ProjectInput {
    #[returns(ref)]
    pub bundle: Arc<Bundle>,
    #[returns(copy)]
    pub epoch: u64,
}

/// One unit, as seen with one set of ambient providers: `env` is a digest of what they
/// declared, so a unit is never answered from a memo made against other providers.
#[salsa::interned(debug)]
pub struct SccKey<'db> {
    #[returns(copy)]
    pub id: u64,
    #[returns(copy)]
    pub env: u64,
}

/// A file by the hash of its path, so the same file is the same key in every epoch.
#[salsa::interned(debug)]
pub struct FileRef<'db> {
    #[returns(copy)]
    pub key: u64,
    #[returns(copy)]
    pub env: u64,
}

fn path_key(path: Option<&Path>) -> u64 {
    xxhash_rust::xxh3::xxh3_64(
        path.map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
            .as_bytes(),
    )
}

// What a unit reads of the graph, and nothing else. Two epochs that give a unit equal plans
// give it the same inputs, so its memo carries over a rebuild: a file added elsewhere, an import
// added to another file, a file whose id did not change. A plan holds file ids because outputs
// carry them; an id that moved changes the plan of every unit that sees it.
#[derive(Clone, PartialEq, Eq)]
pub struct SccPlan {
    members: Vec<String>,
    // The members' edges, dynamic order and keys, as text.
    members_graph: String,
    // The units this one may read, with the ids of their files.
    dependencies: Vec<(u64, Vec<u32>)>,
    direct: Vec<u64>,
    environment: String,
}

#[salsa::tracked(returns(clone))]
pub fn scc_plan<'db>(
    db: &'db dyn IncrementalDb,
    project: ProjectInput,
    scc: SccKey<'db>,
) -> Arc<SccPlan> {
    db.note("scc_plan");
    let bundle = Arc::clone(project.bundle(db));
    let Some(&unit) = bundle.scc_by_key.get(&scc.id(db)) else {
        return Arc::new(SccPlan {
            members: Vec::new(),
            members_graph: String::new(),
            dependencies: Vec::new(),
            direct: Vec::new(),
            environment: String::new(),
        });
    };
    let graph = &bundle.graph;
    let mut members = Vec::new();
    let mut detail = String::new();
    for id in &graph.sccs()[unit] {
        let path = graph.files().path(*id);
        members.push(format!(
            "{}:{}:{}:{}:{}",
            id.index(),
            path.map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
            bundle
                .unreadable
                .get(id.index() as usize)
                .copied()
                .unwrap_or(false),
            graph.is_checkable(*id),
            graph.is_dependency_declaration(*id),
        ));
        detail.push_str(&format!(
            "{:?}|{:?}|{:?}|{}|{};",
            graph.edges(*id),
            graph.ordered_dynamic(*id),
            graph.module_key(*id),
            graph.has_syntax_errors(*id),
            bundle.in_cycle.contains(id),
        ));
    }
    let files_of = |dependency: usize| -> (u64, Vec<u32>) {
        (
            bundle.sccs[dependency].key,
            graph.sccs()[dependency]
                .iter()
                .map(|id| id.index())
                .collect(),
        )
    };
    Arc::new(SccPlan {
        members,
        members_graph: detail,
        dependencies: bundle.sccs[unit]
            .dependencies
            .iter()
            .map(|d| files_of(*d))
            .collect(),
        direct: bundle.sccs[unit]
            .direct
            .iter()
            .map(|d| bundle.sccs[*d].key)
            .collect(),
        environment: bundle.environment.clone(),
    })
}

#[salsa::interned(debug)]
pub struct DeclRef<'db> {
    #[returns(copy)]
    pub key: DeclKey,
    #[returns(copy)]
    pub env: u64,
}

#[salsa::tracked(returns(clone))]
pub fn module_header(db: &dyn IncrementalDb, file: SourceFile) -> Arc<ModuleHeader> {
    db.note("module_header");
    let path = file.path(db).to_string_lossy().into_owned();
    let scan = scan_module_requests(file.text(db), &path);
    Arc::new(ModuleHeader {
        requests: scan
            .requests
            .into_iter()
            .map(|r| (r.specifier, r.kind, r.is_type, r.is_import))
            .collect(),
        parse_failed: scan.parse_failed,
        declares_ambient_modules: scan.declares_ambient_modules,
    })
}

#[salsa::tracked(returns(clone), lru = 4096)]
pub fn check_scc<'db>(
    db: &'db dyn IncrementalDb,
    project: ProjectInput,
    scc: SccKey<'db>,
) -> Arc<SccOutput> {
    // The one tracked read of the graph: the unit's plan. Everything else is taken from the
    // epoch's data untracked, and the plan says when that data matters.
    let _plan = scc_plan(db, project, SccKey::new(db, scc.id(db), 0));
    let env = scc.env(db);
    let bundle = current_bundle(db);
    let generation = bundle.generation.load(Ordering::SeqCst);
    let Some(&unit) = bundle.scc_by_key.get(&scc.id(db)) else {
        db.note("check_scc:?");
        return Arc::new(SccOutput::new(UnitResult::default()));
    };
    // Named by its first member's file name, so a test can say which units ran.
    let first = bundle.graph.sccs()[unit]
        .iter()
        .filter_map(|id| bundle.graph.files().path(*id))
        .filter_map(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .min()
        .unwrap_or_default();
    db.note(&format!("check_scc:{first}"));
    // The identity of the unit that ran, for the work accounting: file names are not unique
    // (two `index.ts`), and one unit can run under two provider keys in one check.
    db.note(&format!("scc_run:{:016x}", scc.id(db)));

    // The text of the members, read through the inputs so the query depends on them.
    let members = &bundle.graph.sccs()[unit];
    let mut texts: HashMap<FileId, Arc<str>> = HashMap::new();
    for id in members {
        if let Some(text) = bundle.text(db, *id) {
            texts.insert(*id, text);
        }
    }

    // What the dependencies exported. Taken from the store when the dependency is known to
    // be current; otherwise through its query, which is always right and depends on all of it.
    let mut dependency_exports: Vec<(FileId, Arc<UnitExports>)> = Vec::new();
    for &dependency in &bundle.sccs[unit].dependencies {
        let stored = (bundle.verified[dependency].load(Ordering::SeqCst) == generation)
            .then(|| {
                bundle.outputs[dependency]
                    .read()
                    .unwrap_or_else(|e| e.into_inner())
                    .upgrade()
            })
            .flatten();
        let output = match stored {
            Some(output) => output,
            None => {
                let key = SccKey::new(db, bundle.sccs[dependency].key, env);
                check_scc(db, project, key)
            }
        };
        dependency_exports.extend(output.shared.iter().cloned());
    }
    let refs: Vec<(FileId, &UnitExports)> = dependency_exports
        .iter()
        .map(|(id, exports)| (*id, exports.as_ref()))
        .collect();
    let lookup = move |id: FileId| texts.get(&id).cloned();
    let cancel = db.cancel_token();
    let providers = current_providers(db);
    // Not a debug assertion: a unit checked against providers other than the ones it is keyed
    // by would be stored under the wrong key and answered wrongly later, with nothing to show
    // for it. A loud failure (the driver reports the unit as crashed) is the better outcome.
    assert_eq!(
        providers.env, env,
        "a unit is keyed by the providers it reads"
    );
    let result = check_unit_isolated(
        &bundle.graph,
        unit,
        &bundle.in_cycle,
        &refs,
        &lookup,
        cancel.as_ref(),
        &providers.units,
    );
    // A unit stopped by the caller has a partial answer. Unwind with Salsa's own cancellation
    // payload so the query stores nothing; the next check runs the unit again.
    if cancel
        .as_ref()
        .is_some_and(crate::CancelToken::is_cancelled)
    {
        std::panic::resume_unwind(Box::new(salsa::Cancelled::Local));
    }
    let output = Arc::new(SccOutput::new(result));

    // The owners of the declarations this unit exports are filled in by the driver (`Memo`),
    // for a unit answered from its memo as much as for one that ran.
    publish(&bundle, unit, &output);

    // The dependency is on exactly what the check read: the declarations it looked up and
    // the export lists of the modules it imports directly.
    let mut declarations: Vec<DeclKey> = Vec::new();
    for (report, _) in &output.result.files {
        declarations.extend(report.reads.iter().copied());
    }
    declarations.sort();
    declarations.dedup();
    for key in declarations {
        let owner = bundle
            .owners
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
            .copied();
        if owner.is_some_and(|owner| owner != unit) {
            let _ = decl_shape(db, project, DeclRef::new(db, key, env));
        }
    }
    for &direct in &bundle.sccs[unit].direct {
        for id in &bundle.graph.sccs()[direct] {
            let _ = export_names(
                db,
                project,
                FileRef::new(db, bundle.file_key[id.index() as usize], env),
            );
        }
    }
    output
}

/// What the ambient providers declared in one check, with a digest of it.
pub struct ProviderSet {
    env: u64,
    units: Vec<(FileId, UnitExports)>,
}

impl ProviderSet {
    fn empty() -> Self {
        Self {
            env: 0,
            units: Vec::new(),
        }
    }

    fn new(graph: &ModuleGraph, units: Vec<(FileId, UnitExports)>) -> Self {
        let mut text = String::new();
        for (id, exports) in &units {
            text.push_str(&format!(
                "{}|{}|{:?};",
                id.index(),
                export_signature(exports),
                graph.edges(*id),
            ));
        }
        let env = if units.is_empty() {
            0
        } else {
            // Never 0, which is the key of "no providers".
            xxhash_rust::xxh3::xxh3_64(text.as_bytes()) | 1
        };
        Self { env, units }
    }
}

fn current_providers(db: &dyn IncrementalDb) -> Arc<ProviderSet> {
    db.current_providers()
        .and_then(|data| data.downcast::<ProviderSet>().ok())
        .unwrap_or_else(|| Arc::new(ProviderSet::empty()))
}

/// One ambient provider, checked on its own: what the driver does before its first pass.
pub struct ProviderOutput {
    checked: Checked,
    digest: u128,
}

impl PartialEq for ProviderOutput {
    fn eq(&self, other: &Self) -> bool {
        self.digest == other.digest
    }
}
impl Eq for ProviderOutput {}

#[salsa::tracked(returns(clone))]
pub fn check_provider<'db>(
    db: &'db dyn IncrementalDb,
    project: ProjectInput,
    file: FileRef<'db>,
) -> Option<Arc<ProviderOutput>> {
    let bundle = current_bundle(db);
    let index = bundle.file_by_key.get(&file.key(db)).copied()?;
    let id = FileId::new(index);
    let unit = bundle.file_scc.get(index as usize).copied()?;
    // The plan of its unit says when what it reads of the graph changed.
    let _plan = scc_plan(db, project, SccKey::new(db, bundle.sccs[unit].key, 0));
    db.note(&format!(
        "check_provider:{}",
        bundle
            .graph
            .files()
            .path(id)
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    ));
    let text = bundle.text(db, id);
    let lookup = move |wanted: FileId| (wanted == id).then(|| text.clone()).flatten();
    let cancel = db.cancel_token();
    let checked = check_provider_isolated(&bundle.graph, id, &lookup, cancel.as_ref());
    if cancel
        .as_ref()
        .is_some_and(crate::CancelToken::is_cancelled)
    {
        std::panic::resume_unwind(Box::new(salsa::Cancelled::Local));
    }
    let digest = xxhash_rust::xxh3::xxh3_128(
        format!(
            "{:?}{}",
            checked.0,
            checked.1.as_ref().map(export_signature).unwrap_or_default()
        )
        .as_bytes(),
    );
    Some(Arc::new(ProviderOutput { checked, digest }))
}

#[salsa::tracked(returns(clone))]
pub fn decl_shape<'db>(
    db: &'db dyn IncrementalDb,
    project: ProjectInput,
    decl: DeclRef<'db>,
) -> DeclShape {
    db.note("decl_shape");
    let bundle = Arc::clone(project.bundle(db));
    let key = decl.key(db);
    let owner = bundle
        .owners
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(&key)
        .copied();
    let Some(owner) = owner else {
        return DeclShape(None);
    };
    let output = check_scc(
        db,
        project,
        SccKey::new(db, bundle.sccs[owner].key, decl.env(db)),
    );
    let found = output
        .result
        .files
        .iter()
        .filter_map(|(_, exports)| exports.as_ref())
        .flat_map(|exports| exports.decls.iter())
        .find(|candidate| candidate.key == key)
        .cloned();
    DeclShape(found)
}

#[salsa::tracked(returns(clone))]
pub fn export_names<'db>(
    db: &'db dyn IncrementalDb,
    project: ProjectInput,
    file: FileRef<'db>,
) -> Arc<NamesEntry> {
    db.note("export_names");
    let bundle = Arc::clone(project.bundle(db));
    let index = bundle.file_by_key.get(&file.key(db)).copied();
    let id = FileId::new(index.unwrap_or(u32::MAX));
    let unit = index.and_then(|index| bundle.file_scc.get(index as usize).copied());
    let empty = || {
        Arc::new(NamesEntry {
            names: Vec::new(),
            complete: false,
        })
    };
    let Some(unit) = unit else {
        return empty();
    };
    let output = check_scc(
        db,
        project,
        SccKey::new(db, bundle.sccs[unit].key, file.env(db)),
    );
    for (report, exports) in &output.result.files {
        if report.file_id == id {
            return match exports {
                Some(exports) => Arc::new(NamesEntry {
                    names: exports.names.to_vec(),
                    complete: exports.complete,
                }),
                None => empty(),
            };
        }
    }
    empty()
}

#[salsa::tracked(returns(clone))]
pub fn file_report<'db>(
    db: &'db dyn IncrementalDb,
    project: ProjectInput,
    file: FileRef<'db>,
) -> Option<Arc<FileReportView>> {
    db.note("file_report");
    let bundle = Arc::clone(project.bundle(db));
    let index = bundle.file_by_key.get(&file.key(db)).copied()?;
    let id = FileId::new(index);
    let unit = bundle.file_scc.get(index as usize).copied()?;
    let output = check_scc(
        db,
        project,
        SccKey::new(db, bundle.sccs[unit].key, file.env(db)),
    );
    let (report, _) = output
        .result
        .files
        .iter()
        .find(|(report, _)| report.file_id == id)?;
    Some(Arc::new(FileReportView {
        digest: xxhash_rust::xxh3::xxh3_64(format!("{report:?}").as_bytes()),
        report: report.clone(),
    }))
}

// ---------------------------------------------------------------------------------------
// The driver.

/// The answer for each unit of the first pass, from the memos.
struct Memo<'a> {
    base: Mutex<Database>,
    pool: Mutex<Vec<Database>>,
    project: ProjectInput,
    bundle: &'a Bundle,
    // The digest of what the providers declared in this check (0 when there are none).
    env: AtomicU64,
    // The units the driver asked for, by key: the denominator of the work accounting.
    asked: Mutex<HashSet<u64>>,
    // What each answered unit's output digested to: with the epoch and the providers' key,
    // everything the first pass hands the second.
    digests: Mutex<Vec<(u64, u128)>>,
    epoch: u64,
    // Set when the driver asked for a unit this layer could not answer.
    fell_through: std::sync::atomic::AtomicBool,
    // The second pass's last result, kept across checks.
    second: &'a Mutex<Option<(u128, UnitResult)>>,
    // 0: no second pass asked, 1: answered from the kept result, 2: it ran.
    second_state: std::sync::atomic::AtomicU8,
}

impl Memo<'_> {
    // A handle is made once per worker thread and handed back, not made per unit: making
    // one costs more than answering a unit whose inputs did not change.
    fn take_handle(&self) -> Database {
        let pooled = self.pool.lock().unwrap_or_else(|e| e.into_inner()).pop();
        match pooled {
            Some(handle) => handle,
            None => self.base.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        }
    }

    fn give_back(&self, handle: Database) {
        self.pool
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(handle);
    }
}

impl UnitMemo for Memo<'_> {
    fn provider(&self, id: FileId) -> Option<Checked> {
        let handle = self.take_handle();
        let key = FileRef::new(&handle, path_key(self.bundle.graph.files().path(id)), 0);
        let output = check_provider(&handle, self.project, key);
        self.give_back(handle);
        output.map(|output| output.checked.clone())
    }

    fn providers_ready(&self, units: &[(FileId, UnitExports)]) {
        let set = ProviderSet::new(&self.bundle.graph, units.to_vec());
        self.env.store(set.env, Ordering::SeqCst);
        self.base
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_providers(Arc::new(set));
    }

    fn first_pass_fingerprint(&self) -> Option<u128> {
        if self.fell_through.load(Ordering::SeqCst) {
            return None;
        }
        let mut digests = self
            .digests
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        digests.sort_unstable();
        let mut hasher = xxhash_rust::xxh3::Xxh3::new();
        hasher.update(&self.epoch.to_le_bytes());
        hasher.update(&self.env.load(Ordering::SeqCst).to_le_bytes());
        hasher.update(&(digests.len() as u64).to_le_bytes());
        for (key, digest) in digests {
            hasher.update(&key.to_le_bytes());
            hasher.update(&digest.to_le_bytes());
        }
        Some(hasher.digest128())
    }

    fn second_pass(&self, fingerprint: u128) -> Option<UnitResult> {
        let kept = self.second.lock().unwrap_or_else(|e| e.into_inner());
        let (held, result) = kept.as_ref()?;
        (*held == fingerprint).then(|| {
            self.second_state.store(1, Ordering::SeqCst);
            result.clone()
        })
    }

    fn keep_second_pass(&self, fingerprint: u128, result: &UnitResult) {
        self.second_state.store(2, Ordering::SeqCst);
        *self.second.lock().unwrap_or_else(|e| e.into_inner()) =
            Some((fingerprint, result.clone()));
    }

    fn unit(&self, unit: usize) -> Option<UnitResult> {
        let Some(info) = self.bundle.sccs.get(unit) else {
            self.fell_through.store(true, Ordering::SeqCst);
            return None;
        };
        let unit_key = info.key;
        self.asked
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(unit_key);
        let handle = self.take_handle();
        let key = SccKey::new(&handle, unit_key, self.env.load(Ordering::SeqCst));
        let output = check_scc(&handle, self.project, key);
        publish(self.bundle, unit, &output);
        self.digests
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((unit_key, output.digest));
        self.bundle.verified[unit].store(
            self.bundle.generation.load(Ordering::SeqCst),
            Ordering::SeqCst,
        );
        let result = output.result.clone();
        self.give_back(handle);
        Some(result)
    }
}

// Makes a unit's output readable by the units that import it, in the epoch's store.
fn publish(bundle: &Bundle, unit: usize, output: &Arc<SccOutput>) {
    *bundle.outputs[unit]
        .write()
        .unwrap_or_else(|e| e.into_inner()) = Arc::downgrade(output);
    let mut owners = bundle.owners.write().unwrap_or_else(|e| e.into_inner());
    for (_, exports) in &output.result.files {
        if let Some(exports) = exports {
            for decl in &exports.decls {
                owners.insert(decl.key, unit);
            }
        }
    }
}

fn current_bundle(db: &dyn IncrementalDb) -> Arc<Bundle> {
    db.current_project()
        .and_then(|data| data.downcast::<Bundle>().ok())
        .expect("a check runs only after the project data is set")
}

/// What one `check` did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CheckStats {
    /// Units answered by the memo, without running the checker.
    pub units_reused: usize,
    /// Distinct units the checker ran for, among the units the driver asked for. A unit
    /// counts once however many times or under however many keys it ran.
    pub units_checked: usize,
    /// Units the driver asked for. Always `units_checked + units_reused`. A unit with no
    /// file to check (a provider, a file that is not checkable) is never asked for and is in
    /// neither count.
    pub units_total: usize,
    /// Whether the graph was rebuilt since the last check: by this check's refresh or by a
    /// `rebuild` the caller made in between (the epoch moved).
    pub rebuilt: bool,
    /// Whether anything a watcher should report changed: the graph was rebuilt, an input's
    /// bytes changed, or the report differs from the last one that was returned. False for a
    /// save without edits, and for a touch that left the bytes alone.
    pub changed: bool,
    /// How many ambient providers the project has: declaration files with `declare module`,
    /// which the driver checks before everything else.
    pub ambient_providers: usize,
    /// How many of those were checked rather than answered from their memo.
    pub providers_checked: usize,
    /// What the second pass (the one that supplies globals, augmentations and ambient
    /// modules) did. It is work the unit counts do not show.
    pub second_pass: SecondPass,
}

/// What the second pass did in one check.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SecondPass {
    /// The project needs none, or the check had no memo to ask.
    #[default]
    NotNeeded,
    /// Its inputs were what they were last time and the kept result was used.
    Reused,
    /// It ran.
    Ran,
}

/// A project checked incrementally. Files are read from disk; `refresh` (called by `check`)
/// finds what changed since the last check.
pub struct IncrementalProject {
    db: Database,
    config: Option<ProjectConfig>,
    entries: Vec<PathBuf>,
    project: ProjectInput,
    epoch: u64,
    generation: u64,
    states: BTreeMap<PathBuf, FileState>,
    headers: HashMap<PathBuf, Arc<ModuleHeader>>,
    unreadable: HashMap<PathBuf, bool>,
    last: CheckStats,
    last_log: Vec<String>,
    // The digest of the providers' declarations in the last check.
    env: u64,
    // Text the editor holds that is not on disk. It wins over the file, for discovery and checking.
    buffers: BTreeMap<PathBuf, Arc<str>>,
    // Paths whose buffer was set or closed since the last refresh: read them again.
    dirty: std::collections::BTreeSet<PathBuf>,
    // Paths of the graph that could not be read when the graph was built. Still unreadable is
    // not news; without this a file that stays unreadable would rebuild the graph on every
    // refresh, which a watcher runs twice a second.
    missing: std::collections::HashSet<PathBuf>,
    // How many inputs had their bytes changed by the last refresh.
    edited: usize,
    // The epoch and the report digest at the last check that returned a report, so the next
    // one can say whether the epoch moved in between and whether the report differs.
    checked_epoch: u64,
    last_digest: Option<u128>,
    // The second pass's result and the fingerprint of what it read; see `UnitMemo`.
    second_cache: Mutex<Option<(u128, UnitResult)>>,
}

impl IncrementalProject {
    /// Opens the project a tsconfig.json describes, like `tsox --project`.
    pub fn open(tsconfig: &Path) -> Result<Self, String> {
        let absolute = std::fs::canonicalize(tsconfig)
            .map_err(|error| format!("could not resolve {}: {error}", tsconfig.display()))?;
        let config = ProjectConfig::load(&absolute).map_err(|error| error.to_string())?;
        Self::build(Some(config), Vec::new())
    }

    /// Opens a project from entry files alone: no tsconfig, no root.
    pub fn open_entries(entries: Vec<PathBuf>) -> Result<Self, String> {
        Self::build(None, entries)
    }

    fn build(config: Option<ProjectConfig>, entries: Vec<PathBuf>) -> Result<Self, String> {
        let mut db = Database::new();
        let no_buffers = BTreeMap::new();
        let (graph, entries) = Self::graph_for(config.as_ref(), &entries, &no_buffers)?;
        let (bundle, states, headers, unreadable, missing) =
            Self::bundle_for(&mut db, graph, &no_buffers)?;
        let bundle = Arc::new(bundle);
        db.set_current_project(Arc::clone(&bundle) as Arc<dyn std::any::Any + Send + Sync>);
        let project = ProjectInput::new(&db, bundle, 0);
        Ok(Self {
            db,
            config,
            entries,
            project,
            epoch: 0,
            generation: 0,
            states,
            headers,
            unreadable,
            last: CheckStats::default(),
            last_log: Vec::new(),
            env: 0,
            buffers: BTreeMap::new(),
            dirty: std::collections::BTreeSet::new(),
            missing,
            edited: 0,
            checked_epoch: 0,
            last_digest: None,
            second_cache: Mutex::new(None),
        })
    }

    fn graph_for(
        config: Option<&ProjectConfig>,
        fixed: &[PathBuf],
        buffers: &BTreeMap<PathBuf, Arc<str>>,
    ) -> Result<(ModuleGraph, Vec<PathBuf>), String> {
        let (resolver, root) = match config {
            Some(config) => (
                ModuleResolver::with_tsconfig(config.config_file())
                    .with_node_next(config.uses_node_next()),
                Some(config.root().to_path_buf()),
            ),
            None => (ModuleResolver::new(), None),
        };
        let mut entries: Vec<PathBuf> = match config {
            Some(config) => config.project_entries(&resolver),
            None => fixed.to_vec(),
        };
        entries = entries
            .into_iter()
            .map(|path| std::fs::canonicalize(&path).unwrap_or(path))
            .collect();
        let overlay = |path: &Path| buffers.get(path).cloned();
        let mut graph = ModuleGraph::build_with_overlay(&entries, &resolver, &overlay)
            .map_err(|e| e.to_string())?;
        if let (Some(config), Some(root)) = (config, root) {
            graph = graph
                .with_root(root)
                .with_config(config)
                .with_type_entries_loaded();
        }
        Ok((graph, entries))
    }

    #[allow(clippy::type_complexity)]
    fn bundle_for(
        db: &mut Database,
        graph: ModuleGraph,
        buffers: &BTreeMap<PathBuf, Arc<str>>,
    ) -> Result<
        (
            Bundle,
            BTreeMap<PathBuf, FileState>,
            HashMap<PathBuf, Arc<ModuleHeader>>,
            HashMap<PathBuf, bool>,
            std::collections::HashSet<PathBuf>,
        ),
        String,
    > {
        let mut sources = Vec::with_capacity(graph.len());
        let mut unreadable = Vec::with_capacity(graph.len());
        let mut states = BTreeMap::new();
        let mut headers = HashMap::new();
        let mut unreadable_by_path = HashMap::new();
        let mut missing = std::collections::HashSet::new();
        for index in 0..graph.len() {
            let id = FileId::new(index as u32);
            let path = graph
                .files()
                .path(id)
                .ok_or_else(|| format!("no path for file {index}"))?
                .to_path_buf();
            let (text, valid, state) = read_source(&path, buffers);
            let kind = if graph.is_dependency_declaration(id) {
                SourceKind::Dependency
            } else {
                SourceKind::Project
            };
            let hash = state.as_ref().map_or(0, |(_, hash)| *hash);
            let file = db.upsert_path(&path, text, hash, kind);
            headers.insert(path.clone(), module_header(&*db, file));
            if let Some((fingerprint_state, _)) = state {
                states.insert(path.clone(), fingerprint_state);
            } else {
                missing.insert(path.clone());
            }
            unreadable.push(!valid);
            unreadable_by_path.insert(path, !valid);
            sources.push(file);
        }
        Ok((
            Bundle::new(graph, sources, unreadable),
            states,
            headers,
            unreadable_by_path,
            missing,
        ))
    }

    /// Reads what changed on disk since the last check. A file whose bytes are the same
    /// starts nothing. A change that touches a file's header, a config file, or a file's
    /// existence rebuilds the graph; any other edit is only new text.
    pub fn refresh(&mut self) -> Result<bool, String> {
        let bundle = Arc::clone(self.project.bundle(&self.db));
        let mut rebuild = bundle.graph.config_changed();
        let mut edits: Vec<(PathBuf, Arc<str>, u64, bool)> = Vec::new();
        // What was seen on disk. Kept apart from `self.states` until the refresh has
        // succeeded: recording a file as seen before its change reached the inputs would, if
        // the rebuild then failed, make the next refresh call the file unchanged and keep the
        // old results for it.
        let mut seen: Vec<(PathBuf, FileState)> = Vec::new();
        for index in 0..bundle.graph.len() {
            let id = FileId::new(index as u32);
            let Some(path) = bundle.graph.files().path(id) else {
                continue;
            };
            // A file with an open buffer is not read from disk, unless its buffer just changed.
            let unchanged = self.buffers.contains_key(path)
                || self
                    .states
                    .get(path)
                    .is_some_and(|state| state.is_unchanged(path));
            if unchanged && !self.dirty.contains(path) {
                continue;
            }
            let (text, valid, state) = read_source(path, &self.buffers);
            let Some((new_state, hash)) = state else {
                // Gone or unreadable. That is a change the first time; a file the graph was
                // built with already unreadable is still the same input.
                if !self.missing.contains(path) {
                    rebuild = true;
                }
                continue;
            };
            seen.push((path.to_path_buf(), new_state));
            edits.push((path.to_path_buf(), text, hash, valid));
        }
        drop(bundle);
        let mut edited = 0;
        if !rebuild {
            // Apply the edits to the inputs, then compare headers.
            for (path, text, hash, valid) in &edits {
                let kind = SourceKind::Project;
                let before = self.db.by_path.get(path).map(|file| file.hash(&self.db));
                let file = self.db.upsert_path(path, Arc::clone(text), *hash, kind);
                if before != Some(*hash) {
                    edited += 1;
                }
                if self.unreadable.get(path) != Some(&!*valid) {
                    rebuild = true;
                }
                let header = module_header(&self.db, file);
                if self.headers.get(path).is_none_or(|old| **old != *header) {
                    rebuild = true;
                }
            }
        }
        if rebuild {
            // On failure nothing is committed: not the seen states, not the dirty set. The
            // next refresh finds the same files changed and tries again.
            self.rebuild()?;
        } else {
            for (path, state) in seen {
                self.states.insert(path, state);
            }
            self.dirty.clear();
        }
        self.edited = edited;
        Ok(rebuild)
    }

    /// Whether the files the configuration selects are not the ones the graph was built from: a
    /// file was added or removed. A directory walk, so a watcher calls it on its own schedule
    /// rather than per keystroke; `check` does not.
    pub fn entries_changed(&self) -> bool {
        let Some(config) = &self.config else {
            return false;
        };
        let resolver = ModuleResolver::with_tsconfig(config.config_file())
            .with_node_next(config.uses_node_next());
        let mut now: Vec<PathBuf> = config
            .project_entries(&resolver)
            .into_iter()
            .map(|path| std::fs::canonicalize(&path).unwrap_or(path))
            .collect();
        now.sort();
        let mut before = self.entries.clone();
        before.sort();
        now != before
    }

    /// Builds the graph again from disk and moves the epoch. For a watcher that saw a file
    /// appear or disappear, or a config change `refresh` cannot see.
    pub fn rebuild(&mut self) -> Result<(), String> {
        if let Some(config) = &self.config {
            let reloaded = ProjectConfig::load(config.config_file()).map_err(|e| e.to_string())?;
            self.config = Some(reloaded);
        }
        let (graph, entries) = Self::graph_for(self.config.as_ref(), &self.entries, &self.buffers)?;
        self.entries = entries;
        let (bundle, states, headers, unreadable, missing) =
            Self::bundle_for(&mut self.db, graph, &self.buffers)?;
        self.dirty.clear();
        self.states = states;
        self.headers = headers;
        self.unreadable = unreadable;
        self.missing = missing;
        self.epoch += 1;
        let bundle = Arc::new(bundle);
        self.db
            .set_current_project(Arc::clone(&bundle) as Arc<dyn std::any::Any + Send + Sync>);
        self.project.set_bundle(&mut self.db).to(bundle);
        self.project.set_epoch(&mut self.db).to(self.epoch);
        Ok(())
    }

    /// Checks the project, reusing every unit whose inputs did not change. The report equals
    /// what `check_project` gives on the same files.
    pub fn check(&mut self) -> Result<ProjectReport, String> {
        self.check_cancellable(&crate::CancelToken::new())
    }

    /// Like `check`, but stops when `token` is cancelled (from another thread, for example when
    /// the file was edited again). A unit that was running is not memoized and runs again in
    /// the next check; the units that finished keep their memos. A cancelled check returns
    /// `Err("the check was cancelled")` rather than a partial report.
    pub fn check_cancellable(
        &mut self,
        token: &crate::CancelToken,
    ) -> Result<ProjectReport, String> {
        if token.is_cancelled() {
            return Err(CANCELLED_CHECK.to_owned());
        }
        let rebuilt = self.refresh()?;
        self.generation += 1;
        let bundle = Arc::clone(self.project.bundle(&self.db));
        bundle.generation.store(self.generation, Ordering::SeqCst);
        // The queries that ran while reading the files belong to this check too.
        let mut log = self.db.take_log();
        let providers = provider_ids(&bundle.graph).len();
        self.db.set_providers(Arc::new(ProviderSet::empty()));

        let snapshot: Vec<Option<Arc<str>>> = (0..bundle.graph.len())
            .map(|index| bundle.text(&self.db, FileId::new(index as u32)))
            .collect();
        let texts = move |id: FileId| snapshot.get(id.index() as usize).cloned().flatten();
        let memo = Memo {
            base: Mutex::new(self.db.clone()),
            pool: Mutex::new(Vec::new()),
            project: self.project,
            bundle: &bundle,
            env: AtomicU64::new(0),
            asked: Mutex::new(HashSet::new()),
            digests: Mutex::new(Vec::new()),
            epoch: self.epoch,
            fell_through: std::sync::atomic::AtomicBool::new(false),
            second: &self.second_cache,
            second_state: std::sync::atomic::AtomicU8::new(0),
        };
        self.db.set_cancel_token(Some(token.clone()));
        // A unit stopped by the token unwinds out of its query; on the driver's own threads that
        // is caught as a crashed unit, but a provider is asked for on this one.
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_project(
                &bundle.graph,
                None,
                Some(token),
                &RunOptions {
                    texts: Some(&texts),
                    memo: Some(&memo),
                },
            )
        }));
        self.env = memo.env.load(Ordering::SeqCst);
        let asked = std::mem::take(&mut *memo.asked.lock().unwrap_or_else(|e| e.into_inner()));
        let second_pass = match memo.second_state.load(Ordering::SeqCst) {
            1 => SecondPass::Reused,
            2 => SecondPass::Ran,
            _ => SecondPass::NotNeeded,
        };
        drop(memo);
        self.db.set_cancel_token(None);
        let report = match outcome {
            Ok(report) => report,
            Err(payload) if token.is_cancelled() => {
                drop(payload);
                self.last_log = log;
                self.last_log.extend(self.db.take_log());
                return Err(CANCELLED_CHECK.to_owned());
            }
            Err(payload) => std::panic::resume_unwind(payload),
        };
        if token.is_cancelled() {
            // Keep what ran; the next check reuses it.
            self.last_log = log;
            self.last_log.extend(self.db.take_log());
            return Err(CANCELLED_CHECK.to_owned());
        }
        log.extend(self.db.take_log());
        // Work is counted per unit, from the identity each run noted: a unit that ran twice in
        // one check (under two provider keys, or once through a dependent's fallback and once
        // through the driver) is one unit checked, and a unit the driver never asked for is
        // not in the denominator. The old count, lines starting with `check_scc`, could exceed
        // the number of units and then reported every other unit as reused.
        let executed: HashSet<u64> = log
            .iter()
            .filter_map(|name| name.strip_prefix("scc_run:"))
            .filter_map(|id| u64::from_str_radix(id, 16).ok())
            .collect();
        let units_checked = asked.intersection(&executed).count();
        let providers_checked = log
            .iter()
            .filter(|name| name.starts_with("check_provider"))
            .count();
        let digest = xxhash_rust::xxh3::xxh3_128(format!("{report:?}").as_bytes());
        let rebuilt = rebuilt || self.epoch != self.checked_epoch;
        self.last = CheckStats {
            units_checked,
            units_reused: asked.len() - units_checked,
            units_total: asked.len(),
            rebuilt,
            changed: rebuilt || self.edited > 0 || self.last_digest != Some(digest),
            ambient_providers: providers,
            providers_checked,
            second_pass,
        };
        self.checked_epoch = self.epoch;
        self.last_digest = Some(digest);
        self.last_log = log;
        Ok(report)
    }

    /// Gives the project the text an editor holds for `path`, which wins over the file on
    /// disk until `close_buffer`. The next `check` treats it like an edit: a change inside a
    /// body re-checks one unit; a change to an import line rebuilds the graph, and discovery
    /// reads the buffer too. A file that exists only as a buffer is not found by an import,
    /// because resolution looks at the disk.
    pub fn set_buffer(&mut self, path: &Path, text: &str) {
        let path = canonical(path);
        self.buffers.insert(path.clone(), Arc::from(text));
        self.dirty.insert(path);
    }

    /// Drops the buffer for `path`; the file on disk is the source again.
    pub fn close_buffer(&mut self, path: &Path) {
        let path = canonical(path);
        if self.buffers.remove(&path).is_some() {
            self.dirty.insert(path);
        }
    }

    /// What the last `check` did.
    pub fn stats(&self) -> CheckStats {
        self.last
    }

    /// The queries that ran since the last call, in order, as `name` or `name:detail`. For tests and tooling.
    pub fn take_log(&mut self) -> Vec<String> {
        let mut log = std::mem::take(&mut self.last_log);
        log.extend(self.db.take_log());
        log
    }

    /// The graph of the current epoch.
    pub fn graph(&self) -> Arc<Bundle> {
        Arc::clone(self.project.bundle(&self.db))
    }

    /// The epoch: how many times the graph was rebuilt.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// The report of one file out of its unit, through the projection query.
    pub fn file_report(&self, path: &Path) -> Option<crate::FileReport> {
        let bundle = self.project.bundle(&self.db);
        let key = path_key(Some(path));
        bundle.file_by_key.get(&key)?;
        let view = file_report(
            &self.db,
            self.project,
            FileRef::new(&self.db, key, self.env),
        )?;
        Some(view.report.clone())
    }

    /// Bounds how many unit outputs Salsa keeps memoized; the rest are recomputed when asked.
    pub fn set_unit_cache_capacity(&mut self, capacity: usize) {
        check_scc::set_lru_capacity(&mut self.db, capacity);
    }

    /// Lets Salsa drop the memos it chose to evict under the LRU limit.
    pub fn evict(&mut self) {
        use salsa::Database as _;
        self.db.trigger_lru_eviction();
    }
}

impl Bundle {
    /// The module graph of this epoch.
    pub fn module_graph(&self) -> &ModuleGraph {
        &self.graph
    }
}

const CANCELLED_CHECK: &str = "the check was cancelled";

/// The path as the graph spells it: canonical when the file or its folder exists.
fn canonical(path: &Path) -> PathBuf {
    if let Ok(path) = std::fs::canonicalize(path) {
        return path;
    }
    match (path.parent().map(std::fs::canonicalize), path.file_name()) {
        (Some(Ok(parent)), Some(name)) => parent.join(name),
        _ => path.to_path_buf(),
    }
}

/// A file's text from its buffer when it has one, else from disk.
fn read_source(
    path: &Path,
    buffers: &BTreeMap<PathBuf, Arc<str>>,
) -> (Arc<str>, bool, Option<(FileState, u64)>) {
    match buffers.get(path) {
        Some(text) => {
            let hash = crate::content_hash(text.as_bytes());
            let fingerprint = crate::FileFingerprint::of(path)
                .unwrap_or_else(|_| crate::FileFingerprint::absent());
            (
                Arc::clone(text),
                true,
                Some((FileState::new(fingerprint, hash), hash)),
            )
        }
        None => read_file(path),
    }
}

fn read_file(path: &Path) -> (Arc<str>, bool, Option<(FileState, u64)>) {
    match std::fs::read(path) {
        Ok(bytes) => {
            let hash = crate::content_hash(&bytes);
            let fingerprint = crate::FileFingerprint::of(path).ok();
            let valid = std::str::from_utf8(&bytes).is_ok();
            let text: Arc<str> = Arc::from(String::from_utf8_lossy(&bytes).as_ref());
            (
                text,
                valid,
                fingerprint.map(|fp| (FileState::new(fp, hash), hash)),
            )
        }
        Err(_) => (Arc::from(""), false, None),
    }
}

#[allow(dead_code)]
fn unused(_: &Checked) {}
