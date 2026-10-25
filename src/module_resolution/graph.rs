use std::borrow::Cow;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::project::ProjectFiles;
use crate::topology::ModuleTopology;
use crate::types::FileId;

use super::config::ProjectConfig;
use super::discovery::{ModuleRequest, ModuleRequestKind, scan_module_requests};
use super::error::ModuleError;
use super::metadata::{FileFingerprint, FileState, content_hash};
use super::resolver::{ModuleResolver, ResolveMode};

// One import or re-export statement, kept whether or not it resolved. `target` is None
// for an import that did not, and it stays in the graph: dropping it would make a file
// with a broken import look like a file with no imports, and a later phase needs the
// specifier to report the missing module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleEdge {
    pub specifier: String,
    pub target: Option<FileId>,
    pub is_type: bool,
    pub is_import: bool,
    pub kind: ModuleRequestKind,
}

// The files reachable from a project's entry points and the imports between them.
// Everything is indexed by FileId, which follows path order (see `in_path_order`), so
// the vectors below are plain arrays and a lookup never hashes a path.
//
// `dependencies` and `dependents` are the resolved edges with duplicates removed (two
// statements importing the same file are one dependency), in both directions, so
// "what must be checked first" and "what is affected by a change" are each one lookup.
// A dynamic import is not one of them: nothing reads what `import("x")` exports yet, so
// ordering by it would only invent cycles. The edge stays in `edges`, which is what the
// walk that finds files follows, so the file is still found, checked and reported.
//
// What a dynamic import does need is a record of who loads whom, since a change to the
// loaded file will matter to its loader once `import("x")` is typed. That is
// `dynamic_dependents`, kept apart from `dependents` so that nothing which orders or
// schedules by `dependents` can pick up an edge that must not order.
//
// `sccs`, `scc_of` and `layers` are worked out once, when the graph is built: the
// components dependencies first (which is also a topological order of them), the
// component of each file, and the files grouped for checking. They are plain arrays, so a
// caller that asks twice does not search twice.
#[derive(Debug)]
pub struct ModuleGraph {
    files: ProjectFiles,
    edges: Vec<Vec<ModuleEdge>>,
    dependencies: Vec<Vec<FileId>>,
    dependents: Vec<Vec<FileId>>,
    dynamic_dependents: Vec<Vec<FileId>>,
    // The dynamic imports a file may wait for and read: see `ordered_dynamic`.
    ordered_dynamic: Vec<Vec<FileId>>,
    sccs: Vec<Vec<FileId>>,
    scc_of: Vec<u32>,
    layers: Vec<Vec<FileId>>,
    states: Vec<FileState>,
    syntax_errors: Vec<bool>,
    // Per file: a script that declares ambient modules, as the discovery parse saw it.
    ambient_scripts: Vec<bool>,
    // The project root, an input of the graph (LLD 3.1.1). None until a caller sets it
    // with `with_root`; without one no file gets a Project key.
    root: Option<PathBuf>,
    // Whether the resolver that built the graph applied a tsconfig.json, so a `paths` or
    // `baseUrl` in it has already had its say in resolution.
    tsconfig_applied: bool,
    // The configuration the graph was built under, once a caller attaches it with
    // `with_config`: an input of the graph like the root.
    config: Option<ConfigInput>,
}

// The config files read for a project, as they were when attached, and whether the
// effective configuration holds something that can answer a package specifier without a
// file the checker reads.
#[derive(Debug)]
struct ConfigInput {
    files: Vec<(PathBuf, FileState)>,
    hides_bare_specifiers: bool,
}

impl ModuleGraph {
    /// Reads every file reachable from `entries` and resolves its imports. Entries must
    /// be absolute so that one file is never interned under two spellings of its path.
    /// The ids of the result follow path order, whatever order the entries were given in.
    pub fn build(entries: &[PathBuf], resolver: &ModuleResolver) -> Result<Self, ModuleError> {
        // Discovery parses every file on the calling thread, and the parser recurses on
        // nested syntax. It runs on the same 64 MB stack the checking workers have, so a
        // file this can parse is a file they can parse, and not on whatever stack the
        // caller happens to be on (8 MB for a main thread, 2 MB for a spawned one).
        let mut graph = crate::stack_guard::on_big_stack(|| Self::discover(entries, resolver))?;
        graph.tsconfig_applied = resolver.reads_tsconfig();
        Ok(graph)
    }

    fn discover(entries: &[PathBuf], resolver: &ModuleResolver) -> Result<Self, ModuleError> {
        // Discovery ids: handed out as files are found, and used only until the walk ends.
        let mut files = ProjectFiles::new();
        for entry in entries {
            if !entry.is_absolute() {
                return Err(ModuleError::RelativeEntry(entry.clone()));
            }
            files.intern(entry.clone());
        }

        let mut edges = Vec::new();
        let mut states = Vec::new();
        let mut syntax_errors = Vec::new();
        let mut ambient_scripts = Vec::new();

        // Files are interned as their importers are scanned, so the list grows while it
        // is walked. Visiting ids in order is a breadth-first walk with no queue.
        let mut next = 0;
        while next < files.len() {
            let id = file_id(next);
            let path = files
                .path(id)
                .expect("every interned id has a path")
                .to_path_buf();

            // The fingerprint is taken before the read. If the file changes in between,
            // the recorded fingerprint is the older one and the change is still seen.
            let fingerprint = FileFingerprint::of(&path).map_err(|source| ModuleError::Read {
                path: path.clone(),
                source,
            })?;
            let bytes = fs::read(&path).map_err(|source| ModuleError::Read {
                path: path.clone(),
                source,
            })?;
            let state = FileState::new(fingerprint, content_hash(&bytes));

            let mut kept: Option<Arc<str>> = None;
            let mut file_edges = Vec::new();
            let mut failed = false;
            let mut declares_ambient = false;
            if is_scannable(&path) {
                let source = String::from_utf8_lossy(&bytes);
                // Borrowed means the bytes were valid UTF-8, which is the only text
                // `read_to_string` would have given the checker. Anything else is not
                // kept, so the checker reads the file itself and fails as before.
                if let Cow::Borrowed(valid) = &source {
                    kept = Some(Arc::from(*valid));
                }
                let scan = scan_module_requests(&source, &path.to_string_lossy());
                failed = scan.parse_failed;
                declares_ambient = scan.declares_ambient_modules;
                for request in scan.requests {
                    let target = resolve_request(resolver, &path, &request)
                        .map(|resolved| files.intern(resolved));
                    file_edges.push(ModuleEdge {
                        specifier: request.specifier,
                        target,
                        is_type: request.is_type,
                        is_import: request.is_import,
                        kind: request.kind,
                    });
                }
            }
            states.push(state.with_text(kept));
            edges.push(file_edges);
            syntax_errors.push(failed);
            ambient_scripts.push(declares_ambient);
            next += 1;
        }

        Ok(Self::in_path_order(
            &files,
            edges,
            states,
            syntax_errors,
            ambient_scripts,
        ))
    }

    // Discovery ids depend on the order the entries came in and on how imports are
    // written, so the same project could be numbered two ways. The ids the graph
    // publishes follow path order instead: the report, the layers and every other list
    // sorted by FileId then come out the same wherever the walk started. Paths compare
    // component by component, so the order does not depend on the separator character.
    // The new id of a file is its place in that order, and every edge target is
    // renumbered to match.
    fn in_path_order(
        discovered: &ProjectFiles,
        edges: Vec<Vec<ModuleEdge>>,
        states: Vec<FileState>,
        syntax_errors: Vec<bool>,
        ambient_scripts: Vec<bool>,
    ) -> Self {
        let paths: Vec<&Path> = (0..discovered.len())
            .map(|position| {
                discovered
                    .path(file_id(position))
                    .expect("every interned id has a path")
            })
            .collect();

        // order[new] is the discovery position of the file that gets the id `new`.
        let mut order: Vec<usize> = (0..paths.len()).collect();
        order.sort_by(|&a, &b| paths[a].cmp(paths[b]));

        let mut new_position = vec![0; order.len()];
        for (new, &old) in order.iter().enumerate() {
            new_position[old] = new;
        }

        let mut files = ProjectFiles::new();
        for &old in &order {
            files.intern(paths[old].to_path_buf());
        }

        let mut edges: Vec<Option<Vec<ModuleEdge>>> = edges.into_iter().map(Some).collect();
        let mut states: Vec<Option<FileState>> = states.into_iter().map(Some).collect();
        let mut sorted_edges = Vec::with_capacity(order.len());
        let mut sorted_states = Vec::with_capacity(order.len());
        let mut sorted_errors = Vec::with_capacity(order.len());
        let mut sorted_ambient = Vec::with_capacity(order.len());
        for &old in &order {
            let mut file_edges = edges[old].take().expect("each file is moved once");
            for edge in &mut file_edges {
                edge.target = edge
                    .target
                    .map(|target| file_id(new_position[index_of(target)]));
            }
            sorted_edges.push(file_edges);
            sorted_states.push(states[old].take().expect("each file is moved once"));
            sorted_errors.push(syntax_errors[old]);
            sorted_ambient.push(ambient_scripts[old]);
        }
        Self::assemble(
            files,
            sorted_edges,
            sorted_states,
            sorted_errors,
            sorted_ambient,
        )
    }

    fn assemble(
        files: ProjectFiles,
        edges: Vec<Vec<ModuleEdge>>,
        states: Vec<FileState>,
        syntax_errors: Vec<bool>,
        ambient_scripts: Vec<bool>,
    ) -> Self {
        let mut dependencies = Vec::with_capacity(edges.len());
        let mut dependents = vec![Vec::new(); edges.len()];
        let mut dynamic_dependents = vec![Vec::new(); edges.len()];
        for (position, file_edges) in edges.iter().enumerate() {
            let mut loaded: Vec<FileId> = file_edges
                .iter()
                .filter(|edge| edge.kind == ModuleRequestKind::DynamicImport)
                .filter_map(|edge| edge.target)
                .collect();
            loaded.sort();
            loaded.dedup();
            for target in &loaded {
                dynamic_dependents[index_of(*target)].push(file_id(position));
            }

            let mut targets: Vec<FileId> = file_edges
                .iter()
                .filter(|edge| edge.kind != ModuleRequestKind::DynamicImport)
                .filter_map(|edge| edge.target)
                .collect();
            targets.sort();
            targets.dedup();
            // Importers are visited in id order, so each dependents list ends up sorted
            // without a sort of its own.
            for target in &targets {
                dependents[index_of(*target)].push(file_id(position));
            }
            dependencies.push(targets);
        }

        let adjacency: Vec<Vec<usize>> = dependencies
            .iter()
            .map(|targets| targets.iter().map(|target| index_of(*target)).collect())
            .collect();
        let ordered_dynamic = acyclic_dynamic_edges(&files, &edges, &adjacency);
        let components = crate::scc::components(&adjacency);
        let scc_of = crate::scc::component_of(adjacency.len(), &components);
        let layers = crate::scc::layers(&adjacency, &components)
            .into_iter()
            .map(|layer| layer.into_iter().map(file_id).collect())
            .collect();
        let sccs = components
            .into_iter()
            .map(|component| component.into_iter().map(file_id).collect())
            .collect();

        Self {
            files,
            edges,
            dependencies,
            dependents,
            dynamic_dependents,
            ordered_dynamic,
            sccs,
            scc_of,
            layers,
            states,
            syntax_errors,
            ambient_scripts,
            root: None,
            tsconfig_applied: false,
            config: None,
        }
    }

    /// Sets the project root that module keys are made relative to. It is an input of
    /// the graph and never part of a key.
    pub fn with_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.root = Some(root.into());
        self
    }

    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// Attaches the project's configuration as an input of the graph: every config file it
    /// was read from is recorded, so `config_changed` can say one has moved, and its
    /// effective options decide whether a missing package can be reported (see
    /// `config_hides_bare_specifiers`).
    pub fn with_config(mut self, config: &ProjectConfig) -> Self {
        let mut files = Vec::new();
        for path in config.sources() {
            if let (Ok(bytes), Ok(fingerprint)) = (fs::read(path), FileFingerprint::of(path)) {
                files.push((
                    path.clone(),
                    FileState::new(fingerprint, content_hash(&bytes)),
                ));
            }
        }
        let has = |name: &str| config.compiler_option(name).is_some();
        // `typeRoots` can bring in `declare module` declarations the checker does not read.
        // `paths` and `baseUrl` can point a package at a file, unless the resolver was given
        // the config and has already followed them. `rootDirs` only applies to relative
        // specifiers, so it cannot answer a package name. `customConditions` and
        // `moduleSuffixes` change which file a package specifier lands on and the resolver
        // is not given either, so a package they would find must not be reported missing.
        let hides_bare_specifiers = has("typeRoots")
            || has("customConditions")
            || has("moduleSuffixes")
            || (!self.tsconfig_applied && (has("paths") || has("baseUrl")));
        self.config = Some(ConfigInput {
            files,
            hides_bare_specifiers,
        });
        self
    }

    /// Whether any config file recorded by `with_config` has changed or gone since. False
    /// when no configuration was attached. A changed config means resolution results, and
    /// the files `include` selects, may be out of date: build the graph again.
    pub fn config_changed(&self) -> bool {
        self.config.as_ref().is_some_and(|config| {
            config
                .files
                .iter()
                .any(|(path, state)| !state.is_unchanged(path))
        })
    }

    /// The config files recorded by `with_config`, the project's own first.
    pub fn config_files(&self) -> Vec<&Path> {
        self.config
            .as_ref()
            .map(|config| {
                config
                    .files
                    .iter()
                    .map(|(path, _)| path.as_path())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// For a graph with a configuration attached: whether it holds something that can
    /// answer a package specifier that no file answers, so a missing package should not
    /// be reported. None when no configuration was attached.
    pub fn config_hides_bare_specifiers(&self) -> Option<bool> {
        self.config
            .as_ref()
            .map(|config| config.hides_bare_specifiers)
    }

    /// Whether the resolver that built this graph applied a `tsconfig.json`.
    pub fn tsconfig_applied(&self) -> bool {
        self.tsconfig_applied
    }

    /// The stable identity of `id` (LLD 3.1.1): Project under the root, Package for a file
    /// inside `node_modules` (named for the package, its version and the path inside it,
    /// not for where it was installed), External for any other file outside the root.
    /// None for a file in `node_modules` whose package has no readable `package.json` with
    /// a name and a version: a wrong key is worse than none.
    pub fn module_key(&self, id: FileId) -> Option<crate::keys::ModuleKey> {
        use crate::keys::ModuleKey;
        let path = self.files.path(id)?;
        if let Some(root) = &self.root {
            if let Some(rel) = ModuleKey::project_path(root, path) {
                return Some(ModuleKey::project(&rel));
            }
        }
        if in_node_modules(path) {
            return package_key(path);
        }
        let text = path.to_str()?.replace('\\', "/");
        Some(ModuleKey::external(&text))
    }

    pub fn files(&self) -> &ProjectFiles {
        &self.files
    }

    pub fn len(&self) -> usize {
        self.edges.len()
    }

    pub fn is_empty(&self) -> bool {
        self.edges.is_empty()
    }

    /// Every import and re-export statement of `id`, in source order.
    pub fn edges(&self, id: FileId) -> &[ModuleEdge] {
        slice_of(&self.edges, id)
    }

    /// The files `id` imports, once each, in id order.
    pub fn dependencies(&self, id: FileId) -> &[FileId] {
        slice_of(&self.dependencies, id)
    }

    /// The files that import `id`, in id order. A dynamic import is not one of them; see
    /// `dynamic_dependents`.
    pub fn dependents(&self, id: FileId) -> &[FileId] {
        slice_of(&self.dependents, id)
    }

    /// The files that load `id` with `import("...")`, in id order, once each. These edges
    /// never order or schedule anything and are not in `dependents`. Whatever has to rerun
    /// when `id` changes is `dependents` together with this list.
    pub fn dynamic_dependents(&self, id: FileId) -> &[FileId] {
        slice_of(&self.dynamic_dependents, id)
    }

    /// The files `id` loads with `import("...")` that the check may wait for, so `id` can read
    /// what they export and type the call. Not in `dependencies`, which stays the graph's own
    /// ordering. A dynamic import is in this list only when the loaded file does not already
    /// reach `id`, directly or through the edges accepted before it, so waiting on it can never
    /// make a cycle. The edges are taken in path order, which does not depend on file ids, so
    /// which edge of a pair of mutual lazy imports is kept is the same however the project was
    /// listed. A dynamic import of a file `id` also imports another way is not listed: that
    /// import orders it already.
    pub fn ordered_dynamic(&self, id: FileId) -> &[FileId] {
        slice_of(&self.ordered_dynamic, id)
    }

    /// The text discovery read for `id`, so a later stage does not read the file again.
    /// None for a file that is not scanned or whose bytes are not valid UTF-8; the caller
    /// then reads the file itself. It is the text the graph's hash and fingerprint
    /// describe, and may be older than the disk: see `changed_files`.
    pub fn source(&self, id: FileId) -> Option<&str> {
        self.states.get(index_of(id)).and_then(FileState::text)
    }

    /// Whether the parser reported a syntax error in `id`. Such a file may have fewer
    /// edges than its source suggests.
    pub fn has_syntax_errors(&self, id: FileId) -> bool {
        self.syntax_errors
            .get(index_of(id))
            .is_some_and(|failed| *failed)
    }

    /// Whether `id` is a script, with no `import` or `export`, that declares ambient
    /// modules with `declare module "x"`. Known from the discovery parse.
    pub fn declares_ambient_modules(&self, id: FileId) -> bool {
        self.ambient_scripts
            .get(index_of(id))
            .is_some_and(|declares| *declares)
    }

    pub fn unresolved(&self) -> impl Iterator<Item = (FileId, &ModuleEdge)> {
        self.edges
            .iter()
            .enumerate()
            .flat_map(|(position, file_edges)| {
                file_edges
                    .iter()
                    .filter(|edge| edge.target.is_none())
                    .map(move |edge| (file_id(position), edge))
            })
    }

    /// Whether the checker should be run on `id`: TypeScript source that is not inside
    /// `node_modules`. Declaration files of dependencies are in the graph because
    /// their imports matter, but checking someone else's package is not this
    /// project's job.
    pub fn is_checkable(&self, id: FileId) -> bool {
        self.files
            .path(id)
            .is_some_and(|path| is_typescript(path) && !in_node_modules(path))
    }

    /// A declaration file inside `node_modules`: checked so that what it exports can be
    /// imported, but never reported on, since checking someone else's package is not this
    /// project's job.
    pub fn is_dependency_declaration(&self, id: FileId) -> bool {
        self.files.path(id).is_some_and(|path| {
            in_node_modules(path)
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.ends_with(".d.ts")
                            || name.ends_with(".d.mts")
                            || name.ends_with(".d.cts")
                    })
        })
    }

    /// The files whose contents differ from what the graph read. A rewrite that leaves
    /// the bytes alone is not a change.
    pub fn changed_files(&self) -> Vec<FileId> {
        (0..self.states.len())
            .map(file_id)
            .filter(|id| {
                self.files
                    .path(*id)
                    .is_none_or(|path| !self.states[index_of(*id)].is_unchanged(path))
            })
            .collect()
    }

    /// The import edges as a petgraph graph. A snapshot of the graph as it is now; it
    /// does not follow changes made to the graph later.
    pub fn topology(&self) -> ModuleTopology {
        let mut topology = ModuleTopology::new();
        for position in 0..self.len() {
            let id = file_id(position);
            topology.add_file(id);
            for edge in &self.edges[index_of(id)] {
                // The same edges that order the graph, so both agree on cycles.
                if edge.kind == ModuleRequestKind::DynamicImport {
                    continue;
                }
                if let Some(target) = edge.target {
                    topology.add_typed_dependency(id, target, edge.kind);
                }
            }
        }
        topology
    }

    /// Groups of files that import each other, directly or through others. A file that
    /// imports itself is a group of one. Members are sorted, and so are the groups.
    pub fn cycles(&self) -> Vec<Vec<FileId>> {
        let mut cycles: Vec<Vec<FileId>> = self
            .sccs
            .iter()
            .filter(|component| {
                component.len() > 1 || self.dependencies(component[0]).contains(&component[0])
            })
            .cloned()
            .collect();
        cycles.sort();
        cycles
    }

    /// Every strongly connected component, a file that is in no cycle being a component of
    /// one. Dependencies first: a component comes after every component it imports from,
    /// so the list is also a topological order. Members are sorted.
    pub fn sccs(&self) -> &[Vec<FileId>] {
        &self.sccs
    }

    /// The position of the component of `id` in `sccs`.
    pub fn scc_of(&self, id: FileId) -> usize {
        self.scc_of
            .get(index_of(id))
            .map_or(0, |index| *index as usize)
    }

    /// A stable identity for the cycle made of `members`, from the keys of its members and
    /// not from their order or from where the project lives: the same files in any order,
    /// under any root, give the same key, and adding or removing a member changes it. None
    /// if a member has no module key (see `module_key`), since a wrong key is worse than none.
    pub fn cycle_key(&self, members: &[FileId]) -> Option<crate::keys::CycleKey> {
        let keys: Option<Vec<crate::keys::ModuleKey>> = members
            .iter()
            .map(|member| self.module_key(*member))
            .collect();
        Some(crate::keys::CycleKey::of(&keys?))
    }

    /// Files grouped so that everything in a layer depends only on earlier layers,
    /// except for imports among members of one cycle, which share a layer. A file
    /// that merely imports a cycle comes after it and is not part of it.
    pub fn layers(&self) -> Vec<Vec<FileId>> {
        self.layers.clone()
    }
}

// A value import is resolved the way a runtime would, falling back to declaration
// files when that finds nothing, because a package can ship types and no code. A type
// import skips the first step: its target is always a declaration, so whichever
// JavaScript entry the plain resolver would pick is the wrong file.
fn resolve_request(
    resolver: &ModuleResolver,
    from: &Path,
    request: &ModuleRequest,
) -> Option<PathBuf> {
    let mode = resolve_mode(resolver, from, request.kind);
    if request.is_type {
        return resolver
            .resolve_dts_from_file_as(mode, from, &request.specifier)
            .ok();
    }
    resolver
        .resolve_from_file_as(mode, from, &request.specifier)
        .or_else(|_| resolver.resolve_dts_from_file_as(mode, from, &request.specifier))
        .ok()
}

// A package's `exports` are read under `require` for `import x = require()`, for any import
// in a `.cts`/`.cjs` file, and, when the resolver follows NodeNext, for an import in a
// `.ts`/`.tsx`/`.js`/`.jsx` file whose nearest package.json is not `"type": "module"`.
// A dynamic `import()` and everything else is read under `import`.
fn resolve_mode(resolver: &ModuleResolver, from: &Path, kind: ModuleRequestKind) -> ResolveMode {
    match kind {
        ModuleRequestKind::ImportEquals => return ResolveMode::Require,
        ModuleRequestKind::DynamicImport => return ResolveMode::Import,
        _ => {}
    }
    match extension(from) {
        Some("cts" | "cjs") => ResolveMode::Require,
        Some("mts" | "mjs") => ResolveMode::Import,
        _ if resolver.is_node_next() && !resolver.nearest_package_is_module(from) => {
            ResolveMode::Require
        }
        _ => ResolveMode::Import,
    }
}

fn extension(path: &Path) -> Option<&str> {
    path.extension().and_then(|ext| ext.to_str())
}

// Resolved imports can land on JSON or other assets. They are nodes, so the importer's
// edge has somewhere to point, but there is no module syntax in them to scan.
fn is_scannable(path: &Path) -> bool {
    matches!(
        extension(path),
        Some("ts" | "tsx" | "mts" | "cts" | "js" | "jsx" | "mjs" | "cjs")
    )
}

fn is_typescript(path: &Path) -> bool {
    matches!(extension(path), Some("ts" | "tsx" | "mts" | "cts"))
}

// The Package key of a file under `node_modules/<name>` or `node_modules/@scope/<name>`:
// the package's own name and version from its `package.json`, and the path of the file
// inside the package. The innermost `node_modules` counts, so a nested install is keyed
// by the package it belongs to.
fn package_key(path: &Path) -> Option<crate::keys::ModuleKey> {
    let parts: Vec<std::path::Component<'_>> = path.components().collect();
    let at = parts
        .iter()
        .rposition(|part| part.as_os_str() == "node_modules")?;
    let first = parts.get(at + 1)?.as_os_str().to_str()?;
    let name_len = if first.starts_with('@') { 2 } else { 1 };
    let end = at + 1 + name_len;
    // The file has to be inside the package directory, not be it.
    if end >= parts.len() {
        return None;
    }
    let name = parts[at + 1..end]
        .iter()
        .map(|part| part.as_os_str().to_str())
        .collect::<Option<Vec<_>>>()?
        .join("/");
    let directory: PathBuf = parts[..end].iter().collect();
    let manifest = std::fs::read_to_string(directory.join("package.json")).ok()?;
    let declared = json_top_level_string(&manifest, "name")?;
    let version = json_top_level_string(&manifest, "version")?;
    // A manifest that names another package than its directory is not trusted to key it.
    if declared != name {
        return None;
    }
    let subpath = parts[end..]
        .iter()
        .map(|part| part.as_os_str().to_str())
        .collect::<Option<Vec<_>>>()?
        .join("/");
    Some(crate::keys::ModuleKey::package(&name, &version, &subpath))
}

// The string value of a key at the top level of a JSON object, read without a JSON
// parser: the project has none, and a manifest is the only JSON it reads. Strings are
// walked with their escapes, and only depth one counts, so a `"version"` inside
// `dependencies` is never mistaken for the package's own. None when the key is missing or
// its value is not a string.
fn json_top_level_string(text: &str, key: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'{' | b'[' => depth += 1,
            b'}' | b']' => depth = depth.saturating_sub(1),
            b'"' => {
                let (value, next) = read_json_string(text, i)?;
                i = next;
                if depth == 1 && value == key {
                    let rest = text[i..].trim_start();
                    let rest = rest.strip_prefix(':')?.trim_start();
                    if rest.starts_with('"') {
                        let offset = text.len() - rest.len();
                        return read_json_string(text, offset).map(|(v, _)| v);
                    }
                    return None;
                }
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

// The string that starts at the quote at `start`, and the index just past its closing
// quote.
fn read_json_string(text: &str, start: usize) -> Option<(String, usize)> {
    let mut out = String::new();
    let mut chars = text[start + 1..].char_indices();
    while let Some((offset, c)) = chars.next() {
        match c {
            '"' => return Some((out, start + 1 + offset + 1)),
            '\\' => match chars.next()?.1 {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                other => out.push(other),
            },
            other => out.push(other),
        }
    }
    None
}

fn in_node_modules(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == "node_modules")
}

fn file_id(index: usize) -> FileId {
    FileId::new(u32::try_from(index).expect("project contains too many files"))
}

fn index_of(id: FileId) -> usize {
    id.index() as usize
}

// The dynamic imports that can be waited for without making a cycle. See `ordered_dynamic`.
fn acyclic_dynamic_edges(
    files: &ProjectFiles,
    edges: &[Vec<ModuleEdge>],
    adjacency: &[Vec<usize>],
) -> Vec<Vec<FileId>> {
    let mut wanted: Vec<(usize, usize)> = Vec::new();
    for (source, file_edges) in edges.iter().enumerate() {
        for edge in file_edges {
            if edge.kind != ModuleRequestKind::DynamicImport {
                continue;
            }
            let Some(target) = edge.target else {
                continue;
            };
            let target = index_of(target);
            if target != source && !adjacency[source].contains(&target) {
                wanted.push((source, target));
            }
        }
    }
    wanted.sort_by(|a, b| {
        let key = |(source, target): &(usize, usize)| {
            (files.path(file_id(*source)), files.path(file_id(*target)))
        };
        key(a).cmp(&key(b)).then(a.cmp(b))
    });
    wanted.dedup();
    let mut reach: Vec<Vec<usize>> = adjacency.to_vec();
    let mut accepted: Vec<Vec<FileId>> = vec![Vec::new(); edges.len()];
    for (source, target) in wanted {
        if reaches(&reach, target, source) {
            continue;
        }
        reach[source].push(target);
        accepted[source].push(file_id(target));
    }
    for list in &mut accepted {
        list.sort();
    }
    accepted
}

// Whether `to` can be reached from `from` along `adjacency`.
fn reaches(adjacency: &[Vec<usize>], from: usize, to: usize) -> bool {
    let mut seen = vec![false; adjacency.len()];
    let mut stack = vec![from];
    while let Some(node) = stack.pop() {
        if node == to {
            return true;
        }
        if std::mem::replace(&mut seen[node], true) {
            continue;
        }
        stack.extend(adjacency[node].iter().copied().filter(|next| !seen[*next]));
    }
    false
}

fn slice_of<T>(table: &[Vec<T>], id: FileId) -> &[T] {
    table.get(index_of(id)).map(Vec::as_slice).unwrap_or(&[])
}

#[cfg(test)]
mod tests {
    use super::{ModuleEdge, ModuleGraph, ModuleRequestKind, file_id};
    use crate::project::ProjectFiles;
    use crate::types::FileId;

    // A graph over `count` files named n0.ts, n1.ts, ... with the given import pairs,
    // for the algorithms, which do not care where an edge came from.
    fn graph(count: usize, imports: &[(usize, usize)]) -> ModuleGraph {
        let mut files = ProjectFiles::new();
        for position in 0..count {
            files.intern(format!("n{position}.ts"));
        }
        let mut edges = vec![Vec::new(); count];
        for (from, to) in imports {
            edges[*from].push(ModuleEdge {
                specifier: format!("./n{to}"),
                target: Some(file_id(*to)),
                is_type: false,
                is_import: true,
                kind: ModuleRequestKind::StaticImport,
            });
        }
        let states = Vec::new();
        ModuleGraph::assemble(files, edges, states, vec![false; count], vec![false; count])
    }

    fn ids(indexes: &[usize]) -> Vec<FileId> {
        indexes.iter().map(|index| file_id(*index)).collect()
    }

    #[test]
    fn a_long_import_chain_does_not_overflow_the_stack() {
        let length = 50_000;
        let imports: Vec<_> = (0..length - 1).map(|n| (n, n + 1)).collect();
        let graph = graph(length, &imports);

        assert!(graph.cycles().is_empty());
        let layers = graph.layers();
        assert_eq!(layers.len(), length);
        assert_eq!(layers[0], ids(&[length - 1]));
        assert_eq!(layers[length - 1], ids(&[0]));
    }

    #[test]
    fn a_ring_is_one_cycle_and_its_importer_is_not_in_it() {
        // 0 imports the ring 1 -> 2 -> 3 -> 1.
        let graph = graph(4, &[(0, 1), (1, 2), (2, 3), (3, 1)]);
        assert_eq!(graph.cycles(), vec![ids(&[1, 2, 3])]);
    }

    #[test]
    fn a_file_importing_itself_is_a_cycle_of_one() {
        let graph = graph(2, &[(0, 0), (1, 0)]);
        assert_eq!(graph.cycles(), vec![ids(&[0])]);
    }

    #[test]
    fn cycle_members_share_a_layer_and_their_importer_follows() {
        // 0 imports the ring 1 <-> 2, and the ring imports the leaf 3.
        let graph = graph(4, &[(0, 1), (1, 2), (2, 1), (2, 3)]);
        assert_eq!(graph.layers(), vec![ids(&[3]), ids(&[1, 2]), ids(&[0])]);
    }

    #[test]
    fn a_diamond_puts_both_sides_in_one_layer() {
        let graph = graph(4, &[(0, 1), (0, 2), (1, 3), (2, 3)]);
        assert_eq!(graph.layers(), vec![ids(&[3]), ids(&[1, 2]), ids(&[0])]);
    }

    #[test]
    fn dependents_are_the_reverse_of_dependencies() {
        let graph = graph(3, &[(0, 2), (1, 2), (0, 2)]);
        assert_eq!(graph.dependencies(file_id(0)), ids(&[2]));
        assert_eq!(graph.dependents(file_id(2)), ids(&[0, 1]));
        assert!(graph.dependents(file_id(0)).is_empty());
    }
}
