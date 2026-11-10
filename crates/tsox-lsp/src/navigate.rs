//! Go to definition and find references.
//!
//! These read the syntax tree and the binder's scopes and nothing else: no check, no types,
//! and no project state, so they work on a file that has errors and they do not wait for a
//! check. What they cannot know without types is what a property access reaches
//! (`object.member`); a name that resolves by scope, an import, an export, a re-export or a
//! namespace member (`ns.member`) is followed through the files it comes from.
//!
//! A [`Host`] says how to read a file (an open document before the disk) and how to turn an
//! import specifier into a file, so the same code runs against the server and against a fake
//! in a test.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use oxc_ast::AstKind;
use oxc_ast::ast::{
    BindingPattern, Declaration, ExportDefaultDeclarationKind, Expression,
    ImportDeclarationSpecifier, ModuleDeclaration, Program,
};
use oxc_semantic::{Semantic, SemanticBuilder, SymbolId};
use oxc_span::GetSpan;

use crate::analysis::with_program;

/// What navigation needs from its surroundings.
pub trait Host {
    /// The text of a file: the open document if there is one, else the disk.
    fn text(&self, path: &Path) -> Option<Arc<str>>;
    /// The file an import specifier written in `from` names.
    fn resolve(&self, from: &Path, specifier: &str) -> Option<PathBuf>;
}

/// A place: a file and a byte range in it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Hit {
    pub path: PathBuf,
    pub span: (u32, u32),
}

/// How far an import is followed through re-exports before giving up. A cycle of re-exports
/// is cut by this, and by the visited set where there is one.
const MAX_DEPTH: usize = 16;

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Runs `read` on the tree and binder data of `text`.
pub(crate) fn with_semantic<R>(
    path: &Path,
    text: &str,
    read: impl FnOnce(&Program<'_>, &Semantic<'_>) -> R,
) -> Option<R> {
    with_program(text, &file_name(path), |program| {
        let semantic = SemanticBuilder::new()
            .with_build_nodes(true)
            .build(program)
            .semantic;
        read(program, &semantic)
    })
}

/// What is under the cursor, as far as this module tells apart.
#[derive(Debug)]
enum Site {
    /// A name bound in this file.
    Symbol(SymbolId),
    /// The imported name of `import { name } from 'source'`.
    Import {
        source: String,
        name: String,
        local: Option<SymbolId>,
    },
    /// The local name of `export { name } from 'source'`.
    ExportFrom { source: String, name: String },
    /// A module specifier.
    Module(String),
    /// `member` in `ns.member`, where `ns` is `import * as ns from 'source'`.
    NamespaceMember { namespace: SymbolId, name: String },
}

fn contains(span: oxc_span::Span, offset: u32) -> bool {
    span.start <= offset && offset <= span.end
}

/// The source specifier of the import or re-export that holds a node.
fn enclosing_source(semantic: &Semantic<'_>, node: oxc_semantic::NodeId) -> Option<String> {
    semantic
        .nodes()
        .ancestors(node)
        .find_map(|ancestor| match ancestor.kind() {
            AstKind::ImportDeclaration(declaration) => Some(declaration.source.value.to_string()),
            AstKind::ExportFromDeclaration(declaration) => {
                Some(declaration.source.value.to_string())
            }
            _ => None,
        })
}

/// The innermost thing under `offset` that can be followed.
fn site_at(semantic: &Semantic<'_>, offset: u32) -> Option<(oxc_span::Span, Site)> {
    let scoping = semantic.scoping();
    let mut best: Option<(u32, oxc_span::Span, Site)> = None;
    let mut offer = |span: oxc_span::Span, site: Site| {
        let size = span.end - span.start;
        if best
            .as_ref()
            .is_none_or(|(smallest, _, _)| size < *smallest)
        {
            best = Some((size, span, site));
        }
    };
    for node in semantic.nodes().iter() {
        match node.kind() {
            AstKind::IdentifierReference(reference) if contains(reference.span, offset) => {
                if let Some(symbol) = scoping.get_reference(reference.reference_id()).symbol_id() {
                    offer(reference.span, Site::Symbol(symbol));
                }
            }
            AstKind::BindingIdentifier(binding) if contains(binding.span, offset) => {
                if let Some(symbol) = binding.symbol_id.get() {
                    offer(binding.span, Site::Symbol(symbol));
                }
            }
            AstKind::ImportSpecifier(specifier) if contains(specifier.imported.span(), offset) => {
                if let Some(source) = enclosing_source(semantic, node.id()) {
                    offer(
                        specifier.imported.span(),
                        Site::Import {
                            source,
                            name: specifier.imported.name().to_string(),
                            local: specifier.local.symbol_id.get(),
                        },
                    );
                }
            }
            AstKind::ExportSpecifier(specifier) if contains(specifier.local.span(), offset) => {
                // Only a re-export has a source to follow; a local export's name is a
                // reference, which is offered above.
                if let Some(source) = enclosing_source(semantic, node.id()) {
                    offer(
                        specifier.local.span(),
                        Site::ExportFrom {
                            source,
                            name: specifier.local.name().to_string(),
                        },
                    );
                }
            }
            AstKind::ImportDeclaration(declaration)
                if contains(declaration.source.span, offset) =>
            {
                offer(
                    declaration.source.span,
                    Site::Module(declaration.source.value.to_string()),
                );
            }
            AstKind::ExportFromDeclaration(declaration)
                if contains(declaration.source.span, offset) =>
            {
                offer(
                    declaration.source.span,
                    Site::Module(declaration.source.value.to_string()),
                );
            }
            AstKind::ExportAllDeclaration(declaration)
                if contains(declaration.source.span, offset) =>
            {
                offer(
                    declaration.source.span,
                    Site::Module(declaration.source.value.to_string()),
                );
            }
            AstKind::StaticMemberExpression(member) if contains(member.property.span, offset) => {
                if let Expression::Identifier(object) = &member.object {
                    if let Some(namespace) =
                        scoping.get_reference(object.reference_id()).symbol_id()
                    {
                        offer(
                            member.property.span,
                            Site::NamespaceMember {
                                namespace,
                                name: member.property.name.to_string(),
                            },
                        );
                    }
                }
            }
            _ => {}
        }
    }
    best.map(|(_, span, site)| (span, site))
}

fn own_hits(path: &Path, semantic: &Semantic<'_>, symbol: SymbolId) -> Vec<Hit> {
    let scoping = semantic.scoping();
    let mut spans = vec![scoping.symbol_span(symbol)];
    spans.extend(
        scoping
            .symbol_redeclarations(symbol)
            .iter()
            .map(|redeclaration| redeclaration.span),
    );
    let mut seen = HashSet::new();
    spans
        .into_iter()
        .filter(|span| seen.insert((span.start, span.end)))
        .map(|span| Hit {
            path: path.to_path_buf(),
            span: (span.start, span.end),
        })
        .collect()
}

/// Where a bound name is declared; for one that an import brought in, where the import
/// leads, and the import itself when it leads nowhere.
fn symbol_hits(
    host: &dyn Host,
    path: &Path,
    semantic: &Semantic<'_>,
    symbol: SymbolId,
    depth: usize,
) -> Vec<Hit> {
    let node = semantic.scoping().symbol_declaration(symbol);
    let followed = match semantic.nodes().kind(node) {
        AstKind::ImportSpecifier(specifier) => enclosing_source(semantic, node).map(|source| {
            follow(
                host,
                path,
                &source,
                specifier.imported.name().as_str(),
                depth,
            )
        }),
        AstKind::ImportDefaultSpecifier(_) => enclosing_source(semantic, node)
            .map(|source| follow(host, path, &source, "default", depth)),
        AstKind::ImportNamespaceSpecifier(_) => enclosing_source(semantic, node).map(|source| {
            host.resolve(path, &source)
                .map(|target| {
                    vec![Hit {
                        path: target,
                        span: (0, 0),
                    }]
                })
                .unwrap_or_default()
        }),
        _ => None,
    };
    match followed {
        Some(hits) if !hits.is_empty() => hits,
        _ => own_hits(path, semantic, symbol),
    }
}

/// The declarations of the name `name` exported by the module `source` names, as written in
/// `from`.
fn follow(host: &dyn Host, from: &Path, source: &str, name: &str, depth: usize) -> Vec<Hit> {
    if depth >= MAX_DEPTH {
        return Vec::new();
    }
    let Some(target) = host.resolve(from, source) else {
        return Vec::new();
    };
    let Some(text) = host.text(&target) else {
        return Vec::new();
    };
    with_semantic(&target, &text, |program, semantic| {
        exported(host, &target, program, semantic, name, depth + 1)
    })
    .unwrap_or_default()
}

fn root_symbol(semantic: &Semantic<'_>, name: &str) -> Option<SymbolId> {
    let scoping = semantic.scoping();
    let root = scoping.root_scope_id();
    scoping.symbol_ids().find(|symbol| {
        scoping.symbol_scope_id(*symbol) == root && scoping.symbol_name(*symbol) == name
    })
}

/// The names a binding pattern introduces, with where each is written.
fn pattern_names(pattern: &BindingPattern<'_>, out: &mut Vec<(String, (u32, u32))>) {
    match pattern {
        BindingPattern::BindingIdentifier(id) => {
            out.push((id.name.to_string(), (id.span.start, id.span.end)));
        }
        BindingPattern::ObjectPattern(object) => {
            for property in &object.properties {
                pattern_names(&property.value, out);
            }
            if let Some(rest) = &object.rest {
                pattern_names(&rest.argument, out);
            }
        }
        BindingPattern::ArrayPattern(array) => {
            for element in array.elements.iter().flatten() {
                pattern_names(element, out);
            }
            if let Some(rest) = &array.rest {
                pattern_names(&rest.argument, out);
            }
        }
        BindingPattern::AssignmentPattern(assignment) => pattern_names(&assignment.left, out),
    }
}

/// The names a declaration introduces, with where each is written.
fn declared_names(declaration: &Declaration<'_>) -> Vec<(String, (u32, u32))> {
    let mut out = Vec::new();
    let mut named = |id: &oxc_ast::ast::BindingIdentifier<'_>| {
        out.push((id.name.to_string(), (id.span.start, id.span.end)));
    };
    match declaration {
        Declaration::VariableDeclaration(variables) => {
            let mut names = Vec::new();
            for declarator in &variables.declarations {
                pattern_names(&declarator.id, &mut names);
            }
            return names;
        }
        Declaration::FunctionDeclaration(function) => {
            if let Some(id) = &function.id {
                named(id);
            }
        }
        Declaration::ClassDeclaration(class) => {
            if let Some(id) = &class.id {
                named(id);
            }
        }
        Declaration::TSTypeAliasDeclaration(alias) => named(&alias.id),
        Declaration::TSInterfaceDeclaration(interface) => named(&interface.id),
        Declaration::TSEnumDeclaration(declaration) => named(&declaration.id),
        Declaration::TSNamespaceDeclaration(namespace) => {
            out.push((
                namespace.id.name.to_string(),
                (namespace.id.span.start, namespace.id.span.end),
            ));
        }
        Declaration::TSImportEqualsDeclaration(import) => named(&import.id),
        Declaration::TSExternalModuleDeclaration(_) | Declaration::TSGlobalDeclaration(_) => {}
    }
    out
}

/// Where `name` is declared among the exports of one file, following `export ... from` and
/// `export *` into the files they name. A direct export wins over one reached by `export *`.
fn exported(
    host: &dyn Host,
    path: &Path,
    program: &Program<'_>,
    semantic: &Semantic<'_>,
    name: &str,
    depth: usize,
) -> Vec<Hit> {
    let hit = |span: (u32, u32)| Hit {
        path: path.to_path_buf(),
        span,
    };
    let mut direct = Vec::new();
    let mut stars = Vec::new();
    for statement in &program.body {
        let Some(module) = statement.as_module_declaration() else {
            continue;
        };
        match module {
            ModuleDeclaration::ExportDeclaration(export) => {
                for (declared, span) in declared_names(&export.declaration) {
                    if declared == name {
                        direct.push(hit(span));
                    }
                }
            }
            ModuleDeclaration::ExportNamedDeclaration(export) => {
                for specifier in &export.specifiers {
                    if specifier.exported.name() != name {
                        continue;
                    }
                    let local = specifier.local.name();
                    let found = root_symbol(semantic, local.as_str())
                        .map(|symbol| symbol_hits(host, path, semantic, symbol, depth))
                        .unwrap_or_default();
                    if found.is_empty() {
                        let span = specifier.local.span();
                        direct.push(hit((span.start, span.end)));
                    } else {
                        direct.extend(found);
                    }
                }
            }
            ModuleDeclaration::ExportFromDeclaration(export) => {
                for specifier in &export.specifiers {
                    if specifier.exported.name() != name {
                        continue;
                    }
                    let found = follow(
                        host,
                        path,
                        export.source.value.as_str(),
                        specifier.local.name().as_str(),
                        depth,
                    );
                    if found.is_empty() {
                        let span = specifier.local.span();
                        direct.push(hit((span.start, span.end)));
                    } else {
                        direct.extend(found);
                    }
                }
            }
            ModuleDeclaration::ExportAllDeclaration(export) => match &export.exported {
                Some(namespace) => {
                    if namespace.name() == name {
                        if let Some(target) = host.resolve(path, export.source.value.as_str()) {
                            direct.push(Hit {
                                path: target,
                                span: (0, 0),
                            });
                        }
                    }
                }
                None => stars.push(export.source.value.to_string()),
            },
            ModuleDeclaration::ExportDefaultDeclaration(export) if name == "default" => {
                let span = match &export.declaration {
                    ExportDefaultDeclarationKind::FunctionDeclaration(function) => {
                        function.id.as_ref().map(|id| id.span)
                    }
                    ExportDefaultDeclarationKind::ClassDeclaration(class) => {
                        class.id.as_ref().map(|id| id.span)
                    }
                    ExportDefaultDeclarationKind::TSInterfaceDeclaration(interface) => {
                        Some(interface.id.span)
                    }
                    _ => None,
                };
                match (span, &export.declaration) {
                    (Some(span), _) => direct.push(hit((span.start, span.end))),
                    // `export default foo;` leads to what `foo` is.
                    (None, ExportDefaultDeclarationKind::Identifier(id)) => {
                        let found = semantic
                            .scoping()
                            .get_reference(id.reference_id())
                            .symbol_id()
                            .map(|symbol| symbol_hits(host, path, semantic, symbol, depth))
                            .unwrap_or_default();
                        if found.is_empty() {
                            direct.push(hit((id.span.start, id.span.end)));
                        } else {
                            direct.extend(found);
                        }
                    }
                    (None, other) => {
                        let start = other.span().start;
                        direct.push(hit((start, start)));
                    }
                }
            }
            _ => {}
        }
    }
    if direct.is_empty() {
        for source in stars {
            direct.extend(follow(host, path, &source, name, depth));
        }
    }
    direct
}

fn dedupe(mut hits: Vec<Hit>) -> Vec<Hit> {
    hits.sort();
    hits.dedup();
    hits
}

/// Where the name at `offset` of `text` (the file at `path`) is declared. Several places
/// when it is declared in several (overloads, an interface and a class that merge).
pub fn definition(host: &dyn Host, path: &Path, text: &str, offset: usize) -> Vec<Hit> {
    let Ok(offset) = u32::try_from(offset) else {
        return Vec::new();
    };
    let hits = with_semantic(path, text, |_, semantic| {
        let (_, site) = site_at(semantic, offset)?;
        Some(match site {
            Site::Symbol(symbol) => symbol_hits(host, path, semantic, symbol, 0),
            Site::Import {
                source,
                name,
                local,
            } => {
                let found = follow(host, path, &source, &name, 0);
                if found.is_empty() {
                    local
                        .map(|symbol| own_hits(path, semantic, symbol))
                        .unwrap_or_default()
                } else {
                    found
                }
            }
            Site::ExportFrom { source, name } => follow(host, path, &source, &name, 0),
            Site::Module(source) => host
                .resolve(path, &source)
                .map(|target| {
                    vec![Hit {
                        path: target,
                        span: (0, 0),
                    }]
                })
                .unwrap_or_default(),
            Site::NamespaceMember { namespace, name } => {
                let node = semantic.scoping().symbol_declaration(namespace);
                match (
                    semantic.nodes().kind(node),
                    enclosing_source(semantic, node),
                ) {
                    (AstKind::ImportNamespaceSpecifier(_), Some(source)) => {
                        follow(host, path, &source, &name, 0)
                    }
                    _ => Vec::new(),
                }
            }
        })
    })
    .flatten()
    .unwrap_or_default();
    dedupe(hits)
}

/// The spans of every use of a bound name in one file: each reference to it.
fn reference_spans(semantic: &Semantic<'_>, symbol: SymbolId) -> Vec<(u32, u32)> {
    let scoping = semantic.scoping();
    scoping
        .get_resolved_reference_ids(symbol)
        .iter()
        .map(|id| {
            let node = scoping.get_reference(*id).node_id();
            let span = semantic.nodes().kind(node).span();
            (span.start, span.end)
        })
        .collect()
}

/// The names under which the file exports the symbol.
fn export_names(program: &Program<'_>, semantic: &Semantic<'_>, symbol: SymbolId) -> Vec<String> {
    let scoping = semantic.scoping();
    let declared = scoping.symbol_span(symbol);
    let mut names = Vec::new();
    for statement in &program.body {
        let Some(module) = statement.as_module_declaration() else {
            continue;
        };
        match module {
            ModuleDeclaration::ExportDeclaration(export) => {
                for (name, span) in declared_names(&export.declaration) {
                    if span == (declared.start, declared.end) {
                        names.push(name);
                    }
                }
            }
            ModuleDeclaration::ExportNamedDeclaration(export) => {
                for specifier in &export.specifiers {
                    if root_symbol(semantic, specifier.local.name().as_str()) == Some(symbol) {
                        names.push(specifier.exported.name().to_string());
                    }
                }
            }
            ModuleDeclaration::ExportDefaultDeclaration(export) => {
                let is_it = match &export.declaration {
                    ExportDefaultDeclarationKind::FunctionDeclaration(function) => {
                        function.id.as_ref().is_some_and(|id| id.span == declared)
                    }
                    ExportDefaultDeclarationKind::ClassDeclaration(class) => {
                        class.id.as_ref().is_some_and(|id| id.span == declared)
                    }
                    ExportDefaultDeclarationKind::Identifier(id) => {
                        scoping.get_reference(id.reference_id()).symbol_id() == Some(symbol)
                    }
                    _ => false,
                };
                if is_it {
                    names.push("default".to_owned());
                }
            }
            _ => {}
        }
    }
    names.sort();
    names.dedup();
    names
}

/// What a scan of one other file found.
#[derive(Default)]
struct Scan {
    hits: Vec<(u32, u32)>,
    /// `(this file, name)` pairs through which the symbol is now also reachable: a name the
    /// file re-exports.
    aliases: Vec<String>,
}

/// The uses, in the file at `path`, of the names in `reached`: each `(module, name)` says that
/// `name` exported by `module` is the symbol being looked for.
fn scan_file(
    host: &dyn Host,
    path: &Path,
    program: &Program<'_>,
    semantic: &Semantic<'_>,
    reached: &HashSet<(PathBuf, String)>,
) -> Scan {
    let scoping = semantic.scoping();
    let mut scan = Scan::default();
    let mut tracked: HashSet<SymbolId> = HashSet::new();
    let is_reached = |module: &Path, name: &str| {
        reached
            .iter()
            .any(|(reached_module, reached_name)| reached_module == module && reached_name == name)
    };

    for statement in &program.body {
        let Some(module) = statement.as_module_declaration() else {
            continue;
        };
        match module {
            ModuleDeclaration::ImportDeclaration(import) => {
                let Some(target) = host.resolve(path, import.source.value.as_str()) else {
                    continue;
                };
                for specifier in import.specifiers.iter().flatten() {
                    match specifier {
                        ImportDeclarationSpecifier::ImportSpecifier(specifier) => {
                            if is_reached(&target, specifier.imported.name().as_str()) {
                                let imported = specifier.imported.span();
                                scan.hits.push((imported.start, imported.end));
                                if let Some(symbol) = specifier.local.symbol_id.get() {
                                    tracked.insert(symbol);
                                    scan.hits.extend(reference_spans(semantic, symbol));
                                }
                            }
                        }
                        ImportDeclarationSpecifier::ImportDefaultSpecifier(specifier) => {
                            if is_reached(&target, "default") {
                                if let Some(symbol) = specifier.local.symbol_id.get() {
                                    tracked.insert(symbol);
                                    scan.hits.extend(reference_spans(semantic, symbol));
                                }
                            }
                        }
                        ImportDeclarationSpecifier::ImportNamespaceSpecifier(specifier) => {
                            let Some(symbol) = specifier.local.symbol_id.get() else {
                                continue;
                            };
                            for id in scoping.get_resolved_reference_ids(symbol) {
                                let node = scoping.get_reference(*id).node_id();
                                if let AstKind::StaticMemberExpression(member) =
                                    semantic.nodes().parent_kind(node)
                                {
                                    if is_reached(&target, member.property.name.as_str()) {
                                        scan.hits.push((
                                            member.property.span.start,
                                            member.property.span.end,
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
            }
            ModuleDeclaration::ExportFromDeclaration(export) => {
                let Some(target) = host.resolve(path, export.source.value.as_str()) else {
                    continue;
                };
                for specifier in &export.specifiers {
                    if is_reached(&target, specifier.local.name().as_str()) {
                        let local = specifier.local.span();
                        scan.hits.push((local.start, local.end));
                        scan.aliases.push(specifier.exported.name().to_string());
                    }
                }
            }
            ModuleDeclaration::ExportAllDeclaration(export) if export.exported.is_none() => {
                let Some(target) = host.resolve(path, export.source.value.as_str()) else {
                    continue;
                };
                for (module, name) in reached {
                    if *module == target {
                        scan.aliases.push(name.clone());
                    }
                }
            }
            _ => {}
        }
    }
    // `import { a } from './a'; export { a };` makes `a` reachable from this file too.
    for statement in &program.body {
        if let Some(ModuleDeclaration::ExportNamedDeclaration(export)) =
            statement.as_module_declaration()
        {
            for specifier in &export.specifiers {
                if let Some(symbol) = root_symbol(semantic, specifier.local.name().as_str()) {
                    if tracked.contains(&symbol) {
                        scan.aliases.push(specifier.exported.name().to_string());
                    }
                }
            }
        }
    }
    scan
}

/// Every use of the name at `offset`, in this file and in the files that import it.
/// `files` are the files to look in besides this one (the project's, and open documents).
/// `include_declaration` adds the places the name is declared. `cancel` stops the search
/// between files; what was found so far is returned.
pub fn references(
    host: &dyn Host,
    files: &[PathBuf],
    path: &Path,
    text: &str,
    offset: usize,
    include_declaration: bool,
    cancel: &AtomicBool,
) -> Vec<Hit> {
    let declarations = definition(host, path, text, offset);
    let Some(home) = declarations.first().map(|hit| hit.path.clone()) else {
        return Vec::new();
    };
    let Some(home_text) = host.text(&home) else {
        return declarations;
    };
    let wanted: Vec<(u32, u32)> = declarations
        .iter()
        .filter(|hit| hit.path == home)
        .map(|hit| hit.span)
        .collect();

    // The file that declares it: every use there, and the names it is exported under.
    let mut hits: Vec<Hit> = Vec::new();
    let mut reached: HashSet<(PathBuf, String)> = HashSet::new();
    let found = with_semantic(&home, &home_text, |program, semantic| {
        let scoping = semantic.scoping();
        let mut local = Vec::new();
        let mut exported = Vec::new();
        for symbol in scoping.symbol_ids() {
            let own = own_hits(&home, semantic, symbol);
            if !own.iter().any(|hit| wanted.contains(&hit.span)) {
                continue;
            }
            local.extend(reference_spans(semantic, symbol));
            if include_declaration {
                local.extend(own.iter().map(|hit| hit.span));
            }
            exported.extend(export_names(program, semantic, symbol));
        }
        (local, exported)
    });
    let Some((local, exported)) = found else {
        return declarations;
    };
    hits.extend(local.into_iter().map(|span| Hit {
        path: home.clone(),
        span,
    }));
    for name in exported {
        reached.insert((home.clone(), name));
    }
    if reached.is_empty() {
        return dedupe(hits);
    }

    // The files that import it, and the files that re-export it for others. A re-export makes
    // a new name to look for, so the files are read again until nothing new turns up.
    let mut passes = 0;
    loop {
        passes += 1;
        let mut grew = false;
        for file in files {
            if cancel.load(Ordering::Relaxed) {
                return dedupe(hits);
            }
            if *file == home {
                continue;
            }
            let Some(file_text) = host.text(file) else {
                continue;
            };
            // A file that mentions none of the names cannot use them, unless it passes
            // everything on with `export *`, which names nothing.
            if !file_text.contains("export *")
                && !reached
                    .iter()
                    .any(|(_, name)| name == "default" || file_text.contains(name.as_str()))
            {
                continue;
            }
            let scan = with_semantic(file, &file_text, |program, semantic| {
                scan_file(host, file, program, semantic, &reached)
            });
            let Some(scan) = scan else {
                continue;
            };
            hits.extend(scan.hits.into_iter().map(|span| Hit {
                path: file.clone(),
                span,
            }));
            for alias in scan.aliases {
                if reached.insert((file.clone(), alias)) {
                    grew = true;
                }
            }
        }
        if !grew || passes >= MAX_DEPTH {
            break;
        }
    }
    dedupe(hits)
}

/// How a name is touched at a place.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Access {
    Read,
    Write,
}

/// Every place in this file where the name at `offset` is declared or used, for highlighting.
/// A declaration and an assignment are writes; everything else is a read. Nothing for a name
/// that is not bound in this file (a global, a property).
pub fn highlights(path: &Path, text: &str, offset: usize) -> Vec<((u32, u32), Access)> {
    let Ok(offset) = u32::try_from(offset) else {
        return Vec::new();
    };
    with_semantic(path, text, |_, semantic| {
        let (_, site) = site_at(semantic, offset)?;
        let symbol = match site {
            Site::Symbol(symbol) => symbol,
            Site::Import {
                local: Some(symbol),
                ..
            } => symbol,
            _ => return None,
        };
        let scoping = semantic.scoping();
        let mut out: Vec<((u32, u32), Access)> = own_hits(path, semantic, symbol)
            .into_iter()
            .map(|hit| (hit.span, Access::Write))
            .collect();
        for id in scoping.get_resolved_reference_ids(symbol) {
            let reference = scoping.get_reference(*id);
            let span = semantic.nodes().kind(reference.node_id()).span();
            let access = if reference.is_write() {
                Access::Write
            } else {
                Access::Read
            };
            out.push(((span.start, span.end), access));
        }
        out.sort();
        out.dedup();
        Some(out)
    })
    .flatten()
    .unwrap_or_default()
}

/// Why a rename was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenameError {
    /// There is no name here to rename, or nothing declares it.
    Nothing,
    /// The name is declared in a library or a declaration file, which is not the user's to edit.
    Library,
    /// The new name is not an identifier.
    InvalidName,
    /// The new name is already declared where the old one is.
    Conflict,
}

impl std::fmt::Display for RenameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            RenameError::Nothing => "there is no name here that can be renamed",
            RenameError::Library => "the name is declared in a library, which cannot be renamed",
            RenameError::InvalidName => "the new name is not a valid identifier",
            RenameError::Conflict => "the new name is already declared in that scope",
        })
    }
}

/// The name a rename would start from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prepared {
    pub span: (u32, u32),
    pub name: String,
}

/// One replacement.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Edit {
    pub path: PathBuf,
    pub span: (u32, u32),
    pub new_text: String,
}

const RESERVED: &[&str] = &[
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "new",
    "null",
    "return",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "yield",
    "let",
    "static",
    "await",
    "implements",
    "interface",
    "package",
    "private",
    "protected",
    "public",
];

fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_alphabetic() || first == '_' || first == '$')
        && chars.all(|c| c.is_alphanumeric() || c == '_' || c == '$')
        && !RESERVED.contains(&name)
}

fn in_library(path: &Path) -> bool {
    path.components()
        .any(|part| part.as_os_str() == "node_modules")
        || path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().ends_with(".d.ts"))
}

/// What a rename at a place is of.
struct Plan {
    span: (u32, u32),
    name: String,
    /// Only the binding in this file, and its uses, not what it was imported from: a renamed
    /// alias (`import { a as b }`), default import or namespace import.
    local_only: bool,
    symbol: Option<SymbolId>,
}

fn plan_at(path: &Path, text: &str, offset: usize) -> Option<Plan> {
    let offset = u32::try_from(offset).ok()?;
    with_semantic(path, text, |_, semantic| {
        let (span, site) = site_at(semantic, offset)?;
        let name = text.get(span.start as usize..span.end as usize)?.to_owned();
        let (symbol, local_only) = match site {
            Site::Symbol(symbol) => {
                let node = semantic.scoping().symbol_declaration(symbol);
                let local_only = match semantic.nodes().kind(node) {
                    AstKind::ImportSpecifier(specifier) => {
                        specifier.imported.span() != specifier.local.span
                    }
                    AstKind::ImportDefaultSpecifier(_) | AstKind::ImportNamespaceSpecifier(_) => {
                        true
                    }
                    _ => false,
                };
                (Some(symbol), local_only)
            }
            Site::Import { .. } => (None, false),
            _ => return None,
        };
        Some(Plan {
            span: (span.start, span.end),
            name,
            local_only,
            symbol,
        })
    })
    .flatten()
}

/// The name at `offset` and whether it can be renamed.
pub fn prepare_rename(
    host: &dyn Host,
    path: &Path,
    text: &str,
    offset: usize,
) -> Result<Prepared, RenameError> {
    let plan = plan_at(path, text, offset).ok_or(RenameError::Nothing)?;
    if !plan.local_only {
        let declared = definition(host, path, text, offset);
        if declared.is_empty() || declared.iter().any(|hit| hit.span == (0, 0)) {
            return Err(RenameError::Nothing);
        }
        if declared.iter().any(|hit| in_library(&hit.path)) {
            return Err(RenameError::Library);
        }
    }
    Ok(Prepared {
        span: plan.span,
        name: plan.name,
    })
}

/// How each hit is to be edited: where the name is used through an import's local name, nothing
/// (the import is changed instead); where it is a specifier whose public name must stay, an
/// alias; everywhere else, the new name.
fn classify(
    path: &Path,
    text: &str,
    spans: &[(u32, u32)],
    new_name: &str,
    skip_import_uses: bool,
) -> Result<Vec<Edit>, RenameError> {
    let found = with_semantic(path, text, |_, semantic| {
        let nodes = semantic.nodes();
        let scoping = semantic.scoping();
        let mut import_specifiers: HashMap<(u32, u32), (bool, String)> = HashMap::new();
        let mut export_specifiers: HashMap<(u32, u32), (bool, String)> = HashMap::new();
        let mut references: HashMap<(u32, u32), SymbolId> = HashMap::new();
        let mut bindings: HashMap<(u32, u32), SymbolId> = HashMap::new();
        for node in nodes.iter() {
            match node.kind() {
                AstKind::ImportSpecifier(specifier) => {
                    let imported = specifier.imported.span();
                    import_specifiers.insert(
                        (imported.start, imported.end),
                        (
                            imported == specifier.local.span,
                            specifier.imported.name().to_string(),
                        ),
                    );
                }
                AstKind::ExportSpecifier(specifier) => {
                    let local = specifier.local.span();
                    export_specifiers.insert(
                        (local.start, local.end),
                        (
                            local == specifier.exported.span(),
                            specifier.local.name().to_string(),
                        ),
                    );
                }
                AstKind::IdentifierReference(reference) => {
                    if let Some(symbol) =
                        scoping.get_reference(reference.reference_id()).symbol_id()
                    {
                        references.insert((reference.span.start, reference.span.end), symbol);
                    }
                }
                AstKind::BindingIdentifier(binding) => {
                    if let Some(symbol) = binding.symbol_id.get() {
                        bindings.insert((binding.span.start, binding.span.end), symbol);
                    }
                }
                _ => {}
            }
        }
        let mut edits = Vec::new();
        for span in spans {
            let plain = || Edit {
                path: path.to_path_buf(),
                span: *span,
                new_text: new_name.to_owned(),
            };
            if let Some((same, old)) = import_specifiers.get(span) {
                edits.push(if *same {
                    Edit {
                        new_text: format!("{new_name} as {old}"),
                        ..plain()
                    }
                } else {
                    plain()
                });
            } else if let Some((same, old)) = export_specifiers.get(span) {
                edits.push(if *same {
                    Edit {
                        new_text: format!("{new_name} as {old}"),
                        ..plain()
                    }
                } else {
                    plain()
                });
            } else if let Some(symbol) = references.get(span) {
                let through_import = matches!(
                    nodes.kind(scoping.symbol_declaration(*symbol)),
                    AstKind::ImportSpecifier(_)
                        | AstKind::ImportDefaultSpecifier(_)
                        | AstKind::ImportNamespaceSpecifier(_)
                );
                if !(skip_import_uses && through_import) {
                    edits.push(plain());
                }
            } else if let Some(symbol) = bindings.get(span) {
                let scope = scoping.symbol_scope_id(*symbol);
                let taken = scoping
                    .get_bindings(scope)
                    .iter()
                    .any(|(name, other)| name.as_str() == new_name && other != symbol);
                if taken {
                    return Err(RenameError::Conflict);
                }
                edits.push(plain());
            } else {
                edits.push(plain());
            }
        }
        Ok(edits)
    });
    found.unwrap_or(Ok(Vec::new()))
}

/// The edits that rename the name at `offset` to `new_name`, across the files that use it.
/// Where another file imports it, only the import (or re-export) is changed: a name used
/// as it was imported gets an alias (`import { renamed as old }`), so the rest of that file is
/// left alone and what a module exports stays what it was called.
pub fn rename(
    host: &dyn Host,
    files: &[PathBuf],
    path: &Path,
    text: &str,
    offset: usize,
    new_name: &str,
    cancel: &AtomicBool,
) -> Result<Vec<Edit>, RenameError> {
    if !is_identifier(new_name) {
        return Err(RenameError::InvalidName);
    }
    let plan = plan_at(path, text, offset).ok_or(RenameError::Nothing)?;
    prepare_rename(host, path, text, offset)?;

    let hits: Vec<Hit> = if plan.local_only {
        let Some(symbol) = plan.symbol else {
            return Err(RenameError::Nothing);
        };
        with_semantic(path, text, |_, semantic| {
            let mut spans: Vec<(u32, u32)> = own_hits(path, semantic, symbol)
                .into_iter()
                .map(|hit| hit.span)
                .collect();
            spans.extend(reference_spans(semantic, symbol));
            spans
        })
        .unwrap_or_default()
        .into_iter()
        .map(|span| Hit {
            path: path.to_path_buf(),
            span,
        })
        .collect()
    } else {
        references(host, files, path, text, offset, true, cancel)
    };

    let mut by_file: HashMap<PathBuf, Vec<(u32, u32)>> = HashMap::new();
    for hit in hits {
        by_file.entry(hit.path).or_default().push(hit.span);
    }
    let mut edits = Vec::new();
    for (file, spans) in by_file {
        let Some(file_text) = host.text(&file) else {
            continue;
        };
        edits.extend(classify(
            &file,
            &file_text,
            &spans,
            new_name,
            !plan.local_only,
        )?);
    }
    edits.sort();
    edits.dedup();
    Ok(edits)
}

/// Texts read once for a whole request, so that every answer is from one view of the files.
pub struct Snapshot<'h> {
    host: &'h dyn Host,
    texts: std::sync::Mutex<HashMap<PathBuf, Option<Arc<str>>>>,
}

impl<'h> Snapshot<'h> {
    pub fn new(host: &'h dyn Host) -> Self {
        Self {
            host,
            texts: std::sync::Mutex::new(HashMap::new()),
        }
    }
}

impl Host for Snapshot<'_> {
    fn text(&self, path: &Path) -> Option<Arc<str>> {
        let mut texts = self.texts.lock().unwrap_or_else(|e| e.into_inner());
        texts
            .entry(path.to_path_buf())
            .or_insert_with(|| self.host.text(path))
            .clone()
    }

    fn resolve(&self, from: &Path, specifier: &str) -> Option<PathBuf> {
        self.host.resolve(from, specifier)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Files in memory; `./x` and `../x` resolve to `x.ts`, `x.tsx` or `x/index.ts`.
    struct Fake {
        files: HashMap<PathBuf, Arc<str>>,
    }

    impl Fake {
        fn new(files: &[(&str, &str)]) -> Self {
            Self {
                files: files
                    .iter()
                    .map(|(name, text)| (PathBuf::from(format!("/p/{name}")), Arc::from(*text)))
                    .collect(),
            }
        }

        fn path(name: &str) -> PathBuf {
            PathBuf::from(format!("/p/{name}"))
        }

        fn all(&self) -> Vec<PathBuf> {
            let mut all: Vec<_> = self.files.keys().cloned().collect();
            all.sort();
            all
        }
    }

    impl Host for Fake {
        fn text(&self, path: &Path) -> Option<Arc<str>> {
            self.files.get(path).cloned()
        }

        fn resolve(&self, from: &Path, specifier: &str) -> Option<PathBuf> {
            if !specifier.starts_with('.') {
                return None;
            }
            let base = from.parent()?.join(specifier);
            let mut normal = PathBuf::new();
            for part in base.components() {
                match part {
                    std::path::Component::ParentDir => {
                        normal.pop();
                    }
                    std::path::Component::CurDir => {}
                    other => normal.push(other),
                }
            }
            for candidate in [
                normal.with_extension("ts"),
                normal.with_extension("tsx"),
                normal.join("index.ts"),
            ] {
                if self.files.contains_key(&candidate) {
                    return Some(candidate);
                }
            }
            None
        }
    }

    /// The offset of the `n`th (from 0) occurrence of `needle`, plus `into` bytes.
    fn at(text: &str, needle: &str, n: usize, into: usize) -> usize {
        text.match_indices(needle).nth(n).expect("needle present").0 + into
    }

    fn slice<'a>(fake: &'a Fake, hit: &Hit) -> &'a str {
        let text = &fake.files[&hit.path];
        &text[hit.span.0 as usize..hit.span.1 as usize]
    }

    fn def(fake: &Fake, file: &str, needle: &str, n: usize) -> Vec<Hit> {
        let path = Fake::path(file);
        let text = fake.files[&path].clone();
        definition(fake, &path, &text, at(&text, needle, n, 0))
    }

    fn refs(fake: &Fake, file: &str, needle: &str, n: usize, decl: bool) -> Vec<Hit> {
        let path = Fake::path(file);
        let text = fake.files[&path].clone();
        references(
            fake,
            &fake.all(),
            &path,
            &text,
            at(&text, needle, n, 0),
            decl,
            &AtomicBool::new(false),
        )
    }

    fn where_(hits: &[Hit]) -> Vec<(String, (u32, u32))> {
        hits.iter()
            .map(|hit| {
                (
                    hit.path.file_name().unwrap().to_string_lossy().into_owned(),
                    hit.span,
                )
            })
            .collect()
    }

    #[test]
    fn a_local_use_leads_to_its_declaration() {
        let fake = Fake::new(&[("a.ts", "const value = 1;\nconsole.log(value);\n")]);
        let hits = def(&fake, "a.ts", "value", 1);
        assert_eq!(hits.len(), 1);
        assert_eq!(slice(&fake, &hits[0]), "value");
        assert_eq!(hits[0].span.0, 6);
    }

    #[test]
    fn the_cursor_may_be_at_either_end_of_the_name() {
        let text = "const value = 1;\nuse(value);\n";
        let fake = Fake::new(&[("a.ts", text)]);
        let path = Fake::path("a.ts");
        let start = at(text, "value", 1, 0);
        for offset in [start, start + 5] {
            assert_eq!(
                definition(&fake, &path, text, offset).len(),
                1,
                "offset {offset}"
            );
        }
    }

    #[test]
    fn a_parameter_shadows_an_outer_name() {
        let fake = Fake::new(&[(
            "a.ts",
            "const x = 1;\nfunction f(x: number) { return x; }\n",
        )]);
        let hits = def(&fake, "a.ts", "x;", 0);
        assert_eq!(hits[0].span.0 as usize, "const x = 1;\nfunction f(".len());
    }

    #[test]
    fn a_type_reference_leads_to_its_interface() {
        let fake = Fake::new(&[("a.ts", "interface Shape { n: number }\nlet s: Shape;\n")]);
        let hits = def(&fake, "a.ts", "Shape", 1);
        assert_eq!(slice(&fake, &hits[0]), "Shape");
        assert_eq!(hits[0].span.0, 10);
    }

    #[test]
    fn overloads_and_merged_declarations_give_every_place() {
        let fake = Fake::new(&[(
            "a.ts",
            "function f(a: string): void;\nfunction f(a: number): void;\nfunction f(a: any) {}\nf(1);\n",
        )]);
        let hits = def(&fake, "a.ts", "f(1)", 0);
        assert_eq!(hits.len(), 3, "{hits:?}");
    }

    #[test]
    fn a_global_has_no_definition() {
        let fake = Fake::new(&[("a.ts", "console.log(1);\n")]);
        assert!(def(&fake, "a.ts", "console", 0).is_empty());
    }

    #[test]
    fn a_property_access_is_not_guessed() {
        let fake = Fake::new(&[("a.ts", "const o = { m: 1 };\no.m;\n")]);
        assert!(def(&fake, "a.ts", "m;", 0).is_empty());
    }

    #[test]
    fn an_imported_name_leads_into_the_file_that_exports_it() {
        let fake = Fake::new(&[
            (
                "lib.ts",
                "export function greet() {}\nexport const k = 1;\n",
            ),
            ("a.ts", "import { greet, k } from './lib';\ngreet(); k;\n"),
        ]);
        let use_site = def(&fake, "a.ts", "greet", 1);
        assert_eq!(where_(&use_site), vec![("lib.ts".to_owned(), (16, 21))]);
        // On the name in the import list itself too.
        let in_import = def(&fake, "a.ts", "greet", 0);
        assert_eq!(in_import, use_site);
        let constant = def(&fake, "a.ts", "k;", 0);
        assert_eq!(slice(&fake, &constant[0]), "k");
    }

    #[test]
    fn an_alias_in_the_import_leads_to_the_original() {
        let fake = Fake::new(&[
            ("lib.ts", "export class Thing {}\n"),
            (
                "a.ts",
                "import { Thing as Other } from './lib';\nnew Other();\n",
            ),
        ]);
        let hits = def(&fake, "a.ts", "Other", 1);
        assert_eq!(slice(&fake, &hits[0]), "Thing");
        assert_eq!(hits[0].path, Fake::path("lib.ts"));
    }

    #[test]
    fn types_and_values_of_one_name_both_count() {
        let fake = Fake::new(&[
            (
                "lib.ts",
                "export interface Id { n: number }\nexport const Id = 1;\n",
            ),
            (
                "a.ts",
                "import { Id } from './lib';\nlet x: Id = Id as any;\n",
            ),
        ]);
        assert_eq!(def(&fake, "a.ts", "Id", 1).len(), 2);
    }

    #[test]
    fn a_default_import_leads_to_the_default_export() {
        let fake = Fake::new(&[
            ("lib.ts", "export default function main() {}\n"),
            ("a.ts", "import run from './lib';\nrun();\n"),
        ]);
        let hits = def(&fake, "a.ts", "run", 1);
        assert_eq!(slice(&fake, &hits[0]), "main");
    }

    #[test]
    fn a_renamed_export_and_a_reexport_chain_are_followed() {
        let fake = Fake::new(&[
            ("deep.ts", "const inner = 1;\nexport { inner as outer };\n"),
            ("mid.ts", "export { outer as middle } from './deep';\n"),
            ("barrel.ts", "export * from './mid';\n"),
            ("a.ts", "import { middle } from './barrel';\nmiddle;\n"),
        ]);
        let hits = def(&fake, "a.ts", "middle", 1);
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].path, Fake::path("deep.ts"));
        assert_eq!(slice(&fake, &hits[0]), "inner");
    }

    #[test]
    fn a_namespace_import_member_leads_to_the_export() {
        let fake = Fake::new(&[
            ("lib.ts", "export const one = 1;\nexport const two = 2;\n"),
            ("a.ts", "import * as lib from './lib';\nlib.two;\n"),
        ]);
        let hits = def(&fake, "a.ts", "two", 0);
        assert_eq!(slice(&fake, &hits[0]), "two");
        assert_eq!(hits[0].path, Fake::path("lib.ts"));
        // The namespace name itself leads to the module.
        let module = def(&fake, "a.ts", "lib", 1);
        assert_eq!(where_(&module), vec![("lib.ts".to_owned(), (0, 0))]);
    }

    #[test]
    fn a_module_specifier_leads_to_the_file() {
        let fake = Fake::new(&[
            ("lib.ts", "export {};\n"),
            ("a.ts", "import './lib';\nexport * from './lib';\n"),
        ]);
        assert_eq!(
            where_(&def(&fake, "a.ts", "'./lib'", 0)),
            vec![("lib.ts".to_owned(), (0, 0))]
        );
        assert_eq!(
            where_(&def(&fake, "a.ts", "'./lib'", 1)),
            vec![("lib.ts".to_owned(), (0, 0))]
        );
    }

    #[test]
    fn an_import_that_does_not_resolve_stays_on_the_import() {
        let fake = Fake::new(&[("a.ts", "import { gone } from './missing';\ngone();\n")]);
        let hits = def(&fake, "a.ts", "gone", 1);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, Fake::path("a.ts"));
        assert_eq!(slice(&fake, &hits[0]), "gone");
    }

    #[test]
    fn a_name_the_target_does_not_export_stays_on_the_import() {
        let fake = Fake::new(&[
            ("lib.ts", "export const other = 1;\n"),
            ("a.ts", "import { nope } from './lib';\nnope;\n"),
        ]);
        let hits = def(&fake, "a.ts", "nope", 1);
        assert_eq!(hits[0].path, Fake::path("a.ts"));
    }

    #[test]
    fn a_cycle_of_reexports_ends() {
        let fake = Fake::new(&[
            ("a.ts", "export * from './b';\n"),
            ("b.ts", "export * from './a';\n"),
            ("c.ts", "import { ghost } from './a';\nghost;\n"),
        ]);
        let hits = def(&fake, "c.ts", "ghost", 1);
        assert_eq!(hits[0].path, Fake::path("c.ts"), "falls back to the import");
    }

    #[test]
    fn definition_works_before_a_fatal_syntax_error() {
        let fake = Fake::new(&[(
            "a.ts",
            "const value = 1;\nuse(value);\nclass Half {\n  m() {\n",
        )]);
        let hits = def(&fake, "a.ts", "value", 1);
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn references_in_one_file_with_and_without_the_declaration() {
        let fake = Fake::new(&[("a.ts", "const v = 1;\nv + v;\n")]);
        assert_eq!(refs(&fake, "a.ts", "v + v", 0, false).len(), 2);
        assert_eq!(refs(&fake, "a.ts", "v + v", 0, true).len(), 3);
    }

    #[test]
    fn references_to_an_export_include_the_importing_files() {
        let fake = Fake::new(&[
            ("lib.ts", "export function greet() {}\ngreet();\n"),
            ("a.ts", "import { greet } from './lib';\ngreet();\n"),
            (
                "b.ts",
                "import { greet as hi } from './lib';\nhi(); hi();\n",
            ),
            ("c.ts", "import { other } from './lib';\ngreet();\n"),
        ]);
        let hits = refs(&fake, "lib.ts", "greet", 0, true);
        let by_file = |name: &str| hits.iter().filter(|h| h.path == Fake::path(name)).count();
        assert_eq!(by_file("lib.ts"), 2, "declaration and the call");
        // The import list entry and the call.
        assert_eq!(by_file("a.ts"), 2);
        // The imported name and two calls through the alias.
        assert_eq!(by_file("b.ts"), 3);
        assert_eq!(by_file("c.ts"), 0, "c.ts imports something else");
    }

    #[test]
    fn references_from_an_importing_file_find_the_other_importers() {
        let fake = Fake::new(&[
            ("lib.ts", "export const k = 1;\n"),
            ("a.ts", "import { k } from './lib';\nk;\n"),
            ("b.ts", "import { k } from './lib';\nk;\n"),
        ]);
        let hits = refs(&fake, "a.ts", "k;", 0, false);
        assert!(hits.iter().any(|h| h.path == Fake::path("b.ts")));
        assert!(hits.iter().any(|h| h.path == Fake::path("a.ts")));
        assert!(
            !hits.iter().any(|h| h.path == Fake::path("lib.ts")),
            "no declaration asked for"
        );
    }

    #[test]
    fn references_follow_a_reexport_to_its_importers() {
        let fake = Fake::new(&[
            ("lib.ts", "export const k = 1;\n"),
            ("barrel.ts", "export { k as kay } from './lib';\n"),
            ("a.ts", "import { kay } from './barrel';\nkay;\n"),
            ("star.ts", "export * from './barrel';\n"),
            ("b.ts", "import { kay } from './star';\nkay;\n"),
        ]);
        let hits = refs(&fake, "lib.ts", "k =", 0, true);
        for file in ["lib.ts", "barrel.ts", "a.ts", "b.ts"] {
            assert!(
                hits.iter().any(|h| h.path == Fake::path(file)),
                "missing {file}: {hits:?}"
            );
        }
    }

    #[test]
    fn references_through_a_namespace_import_count_the_member() {
        let fake = Fake::new(&[
            ("lib.ts", "export const one = 1;\nexport const two = 2;\n"),
            (
                "a.ts",
                "import * as lib from './lib';\nlib.one; lib.two; lib.one;\n",
            ),
        ]);
        let hits = refs(&fake, "lib.ts", "one", 0, false);
        assert_eq!(
            hits.iter().filter(|h| h.path == Fake::path("a.ts")).count(),
            2
        );
    }

    #[test]
    fn references_to_a_default_export_include_default_imports() {
        let fake = Fake::new(&[
            ("lib.ts", "export default function main() {}\n"),
            ("a.ts", "import run from './lib';\nrun();\n"),
        ]);
        let hits = refs(&fake, "lib.ts", "main", 0, true);
        assert!(
            hits.iter().any(|h| h.path == Fake::path("a.ts")),
            "{hits:?}"
        );
    }

    #[test]
    fn a_name_that_is_not_exported_has_references_in_its_file_only() {
        let fake = Fake::new(&[
            ("lib.ts", "const hidden = 1; hidden;\n"),
            ("a.ts", "const hidden = 2; hidden;\n"),
        ]);
        let hits = refs(&fake, "lib.ts", "hidden", 0, true);
        assert!(hits.iter().all(|h| h.path == Fake::path("lib.ts")));
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn a_cancelled_search_returns_what_it_had() {
        let fake = Fake::new(&[
            ("lib.ts", "export const k = 1;\nk;\n"),
            ("a.ts", "import { k } from './lib';\nk;\n"),
        ]);
        let path = Fake::path("lib.ts");
        let text = fake.files[&path].clone();
        let hits = references(
            &fake,
            &fake.all(),
            &path,
            &text,
            at(&text, "k;", 0, 0),
            true,
            &AtomicBool::new(true),
        );
        assert!(
            hits.iter().all(|h| h.path == path),
            "only the home file: {hits:?}"
        );
        assert!(!hits.is_empty());
    }

    fn hl<'a>(text: &'a str, needle: &str, n: usize) -> Vec<(&'a str, Access)> {
        highlights(&Fake::path("a.ts"), text, at(text, needle, n, 0))
            .into_iter()
            .map(|((start, end), access)| (&text[start as usize..end as usize], access))
            .collect()
    }

    #[test]
    fn highlights_tell_writes_from_reads() {
        let text = "let v = 1;\nv = 2;\nconsole.log(v);\n";
        let found = highlights(&Fake::path("a.ts"), text, at(text, "v)", 0, 0));
        assert_eq!(
            found,
            vec![
                ((4, 5), Access::Write),
                ((11, 12), Access::Write),
                (
                    (
                        text.rfind('v').unwrap() as u32,
                        text.rfind('v').unwrap() as u32 + 1
                    ),
                    Access::Read
                ),
            ]
        );
        assert_eq!(hl(text, "console", 0), vec![], "a global");
        assert_eq!(hl("const a = 1; a + a;", "a +", 0).len(), 3);
    }

    /// The files after the edits.
    fn apply(fake: &Fake, edits: &[Edit]) -> HashMap<String, String> {
        let mut out: HashMap<String, String> = HashMap::new();
        for (path, text) in &fake.files {
            let mut mine: Vec<&Edit> = edits.iter().filter(|e| &e.path == path).collect();
            mine.sort_by_key(|e| std::cmp::Reverse(e.span.0));
            let mut text = text.to_string();
            for edit in mine {
                text.replace_range(edit.span.0 as usize..edit.span.1 as usize, &edit.new_text);
            }
            out.insert(
                path.file_name().unwrap().to_string_lossy().into_owned(),
                text,
            );
        }
        out
    }

    fn renamed(
        fake: &Fake,
        file: &str,
        needle: &str,
        n: usize,
        new: &str,
    ) -> Result<HashMap<String, String>, RenameError> {
        let path = Fake::path(file);
        let text = fake.files[&path].clone();
        let edits = rename(
            fake,
            &fake.all(),
            &path,
            &text,
            at(&text, needle, n, 0),
            new,
            &AtomicBool::new(false),
        )?;
        Ok(apply(fake, &edits))
    }

    #[test]
    fn a_local_is_renamed_everywhere_in_its_scope_only() {
        let fake = Fake::new(&[(
            "a.ts",
            "const v = 1;\nfunction f(v: number) { return v; }\nconsole.log(v);\n",
        )]);
        let out = renamed(&fake, "a.ts", "v)", 0, "total").unwrap();
        assert_eq!(
            out["a.ts"],
            "const total = 1;\nfunction f(v: number) { return v; }\nconsole.log(total);\n"
        );
    }

    #[test]
    fn an_export_is_renamed_where_it_is_declared_and_imported_without_touching_uses() {
        let fake = Fake::new(&[
            ("lib.ts", "export function greet() {}\ngreet();\n"),
            ("a.ts", "import { greet } from './lib';\ngreet();\n"),
            ("b.ts", "import { greet as hi } from './lib';\nhi();\n"),
            ("unrelated.ts", "export function greet() {}\ngreet();\n"),
        ]);
        let expected = |out: &HashMap<String, String>| {
            assert_eq!(out["lib.ts"], "export function hello() {}\nhello();\n");
            assert_eq!(
                out["a.ts"],
                "import { hello as greet } from './lib';\ngreet();\n"
            );
            assert_eq!(out["b.ts"], "import { hello as hi } from './lib';\nhi();\n");
            assert_eq!(
                out["unrelated.ts"],
                "export function greet() {}\ngreet();\n"
            );
        };
        expected(&renamed(&fake, "lib.ts", "greet", 0, "hello").unwrap());
        // From a use in an importing file, or from the name in its import list: the same.
        expected(&renamed(&fake, "a.ts", "greet()", 0, "hello").unwrap());
        expected(&renamed(&fake, "a.ts", "greet }", 0, "hello").unwrap());
        expected(&renamed(&fake, "b.ts", "greet as", 0, "hello").unwrap());
    }

    #[test]
    fn renaming_an_alias_changes_only_the_alias() {
        let fake = Fake::new(&[
            ("lib.ts", "export function greet() {}\n"),
            (
                "b.ts",
                "import { greet as hi } from './lib';\nhi(); hi();\n",
            ),
        ]);
        let out = renamed(&fake, "b.ts", "hi()", 0, "short").unwrap();
        assert_eq!(out["lib.ts"], "export function greet() {}\n");
        assert_eq!(
            out["b.ts"],
            "import { greet as short } from './lib';\nshort(); short();\n"
        );
    }

    #[test]
    fn a_default_import_and_a_namespace_import_are_local_names() {
        let fake = Fake::new(&[
            (
                "lib.ts",
                "export default function main() {}\nexport const one = 1;\n",
            ),
            (
                "a.ts",
                "import run from './lib';\nimport * as lib from './lib';\nrun(); lib.one;\n",
            ),
        ]);
        let out = renamed(&fake, "a.ts", "run()", 0, "go").unwrap();
        assert_eq!(out["lib.ts"], fake.files[&Fake::path("lib.ts")].to_string());
        assert!(out["a.ts"].starts_with("import go from './lib';"));
        assert!(out["a.ts"].contains("go();"));
        let out = renamed(&fake, "a.ts", "lib.one", 0, "mod").unwrap();
        assert!(out["a.ts"].contains("import * as mod from './lib';"));
        assert!(out["a.ts"].contains("mod.one;"));
    }

    #[test]
    fn a_namespace_member_is_renamed_with_the_export() {
        let fake = Fake::new(&[
            ("lib.ts", "export const one = 1;\n"),
            ("a.ts", "import * as lib from './lib';\nlib.one;\n"),
        ]);
        let out = renamed(&fake, "lib.ts", "one", 0, "first").unwrap();
        assert_eq!(out["lib.ts"], "export const first = 1;\n");
        assert_eq!(out["a.ts"], "import * as lib from './lib';\nlib.first;\n");
    }

    #[test]
    fn reexports_keep_the_name_they_publish() {
        let fake = Fake::new(&[
            ("lib.ts", "export const k = 1;\n"),
            ("barrel.ts", "export { k } from './lib';\n"),
            ("alias.ts", "export { k as kay } from './lib';\n"),
        ]);
        let out = renamed(&fake, "lib.ts", "k =", 0, "key").unwrap();
        assert_eq!(out["lib.ts"], "export const key = 1;\n");
        assert_eq!(out["barrel.ts"], "export { key as k } from './lib';\n");
        assert_eq!(out["alias.ts"], "export { key as kay } from './lib';\n");
    }

    #[test]
    fn a_local_export_list_keeps_the_public_name() {
        let fake = Fake::new(&[("a.ts", "const a = 1;\nexport { a };\na;\n")]);
        let out = renamed(&fake, "a.ts", "a =", 0, "b").unwrap();
        assert_eq!(out["a.ts"], "const b = 1;\nexport { b as a };\nb;\n");
    }

    #[test]
    fn a_new_name_that_is_not_an_identifier_is_refused() {
        let fake = Fake::new(&[("a.ts", "const a = 1;\n")]);
        for bad in ["", "1x", "a b", "class", "a-b", "let", "a.b"] {
            assert_eq!(
                renamed(&fake, "a.ts", "a =", 0, bad).unwrap_err(),
                RenameError::InvalidName,
                "{bad:?}"
            );
        }
        for good in ["b", "_b", "$b", "b2", "\u{e9}t\u{e9}"] {
            assert!(renamed(&fake, "a.ts", "a =", 0, good).is_ok(), "{good:?}");
        }
    }

    #[test]
    fn a_name_already_declared_there_is_a_conflict() {
        let fake = Fake::new(&[("a.ts", "const a = 1;\nconst b = 2;\na + b;\n")]);
        assert_eq!(
            renamed(&fake, "a.ts", "a =", 0, "b").unwrap_err(),
            RenameError::Conflict
        );
        // Shadowing in an inner scope is not a conflict.
        let fake = Fake::new(&[(
            "a.ts",
            "const a = 1;\nfunction f() { const b = 2; return b; }\na;\n",
        )]);
        assert!(renamed(&fake, "a.ts", "a =", 0, "b").is_ok());
    }

    #[test]
    fn what_a_library_declares_is_not_renamed() {
        let text = "import { x } from './node_modules/pkg';\nx;\n";
        let fake = Fake::new(&[
            ("node_modules/pkg.ts", "export const x = 1;\n"),
            ("a.ts", text),
        ]);
        assert_eq!(
            renamed(&fake, "a.ts", "x;", 0, "y").unwrap_err(),
            RenameError::Library
        );
        assert_eq!(
            prepare_rename(&fake, &Fake::path("a.ts"), text, at(text, "x;", 0, 0)),
            Err(RenameError::Library)
        );
    }

    #[test]
    fn globals_keywords_and_unresolved_names_are_not_renamed() {
        let fake = Fake::new(&[("a.ts", "console.log(1);\nconst a = missing;\n")]);
        for (needle, why) in [
            ("console", "global"),
            ("const", "keyword"),
            ("missing", "unresolved"),
            ("1)", "literal"),
        ] {
            assert_eq!(
                renamed(&fake, "a.ts", needle, 0, "x").unwrap_err(),
                RenameError::Nothing,
                "{why}"
            );
        }
    }

    #[test]
    fn prepare_gives_the_name_and_where_it_is() {
        let text = "const total = 1;\ntotal;\n";
        let fake = Fake::new(&[("a.ts", text)]);
        let prepared =
            prepare_rename(&fake, &Fake::path("a.ts"), text, at(text, "total;", 0, 2)).unwrap();
        assert_eq!(prepared.name, "total");
        assert_eq!(prepared.span, (17, 22));
    }

    #[test]
    fn a_snapshot_reads_each_file_once() {
        use std::sync::atomic::AtomicUsize;
        struct Counting(AtomicUsize);
        impl Host for Counting {
            fn text(&self, _: &Path) -> Option<Arc<str>> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Some(Arc::from("x"))
            }
            fn resolve(&self, _: &Path, _: &str) -> Option<PathBuf> {
                None
            }
        }
        let counting = Counting(AtomicUsize::new(0));
        let snapshot = Snapshot::new(&counting);
        for _ in 0..3 {
            snapshot.text(Path::new("/a.ts"));
        }
        snapshot.text(Path::new("/b.ts"));
        assert_eq!(counting.0.load(Ordering::SeqCst), 2);
    }
}
