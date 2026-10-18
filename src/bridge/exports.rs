// What a unit exports, as portable declarations (LLD 3.4).
//
// Runs inside check_file, after the bodies are checked and before the arena is handed
// back, because a portable declaration is read out of the arena and the arena is cleared
// as soon as the unit ends. Nothing here reports a diagnostic and nothing here changes a
// type: it only reads what the declare pass already resolved.
//
// The file's top-level declarations are listed first, each with the names it is exported
// under, and the exports are built from that list. That is what lets one path serve every
// way of exporting a declaration: `export interface A`, `export default function f`, and
// `export { a, b as c }` naming a declaration written elsewhere in the file.
//
// The table carries interfaces, aliases, classes, enums, functions, variables, namespace
// values, re-exports, overload signatures, destructured bindings, and supported default
// forms. Exports are read after bodies are checked, so ordinary functions and variables
// retain their inferred types. Re-exports preserve the provider's declaration keys instead
// of inventing identities in the forwarding file. A declaration reached by an exported
// signature can travel under its own key without becoming importable by name. Forms whose
// public shape cannot be represented yet keep the table explicitly incomplete; that is
// safer than claiming a missing export is definitely absent.

use oxc_ast::ast::{
    BindingPattern, Class, Declaration, ExportDefaultDeclarationKind, Expression, Function,
    ModuleExportName, Program, Statement, TSTypeParameterDeclaration,
};
use oxc_semantic::{Scoping, SymbolId};
use oxc_span::GetSpan;

use crate::arena::{BUILTIN_GENERICS, DeclSlot, TypeId};
use crate::fxhash::FxHashMap;
use crate::keys::{DeclKey, DeclKind, ModuleKey, TypeParamKey};
use crate::namespace::Resolution;
use crate::portable::{
    DeclInput, Importer, ModuleExports, PNode, PortableDecl, TypeParamInput, export_decl,
};
use crate::stable_walk::KeyResolver;
use crate::types::{ObjectType, PropertyEntry, Type, TypeParameterId};

use super::context::CheckContext;

/// Why a unit's export list is partial, one reason for each way the collector gives up on
/// something. A partial list is safe, because a dependent that sees a name missing from one
/// stays quiet about it, but a reason says what to build next and lets a test pin each case.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IncompleteReason {
    /// The checker panicked on the file, so nothing about it can be trusted.
    CheckerCrashed,
    /// An exported declaration of a kind the portable form does not carry.
    UnsupportedDeclaration,
    /// `export const {} = x`: a pattern that binds no name.
    EmptyBindingPattern,
    /// `declare module "x";` with no body: the module exists and every import from it is any.
    ShorthandAmbientModule,
    /// `export default function () {}` with no body and no return annotation: its return
    /// type would have to be inferred from a body that has no symbol.
    AnonymousDefaultWithoutSignature,
    /// `export = x` where x is not an object value: it can carry properties the model
    /// cannot list.
    NonObjectExportAssignment,
    /// `export { x }` where x is not declared in the file.
    UnknownLocalExport,
    /// An import exported again whose source module has no known exports.
    UnresolvedReExport,
    /// A declaration file with no export statement: tsc exports its unmarked declarations,
    /// and which they are cannot be told from here.
    DeclarationFileWithoutExportStatement,
    /// An exported declaration the portable form could not be built for.
    UnrepresentableDeclaration,
    /// `export * from` a module whose export list was read and is partial. The file is partial
    /// only because that module is.
    StarFromPartialModule,
    /// `export * as ns from` a module whose export list was read and is partial.
    NamespaceStarFromPartialModule,
    /// `export * from` a module that is missing, or that exists but whose exports were not
    /// read. The cause is the module, not a partial list upstream.
    StarFromUnknownModule,
    /// `export * as ns from` a module that is missing, or whose exports were not read.
    NamespaceStarFromUnknownModule,
}

impl IncompleteReason {
    /// A short phrase for a report, in the form "the list is partial because ...".
    pub fn describe(self) -> &'static str {
        match self {
            Self::CheckerCrashed => "the checker crashed on the file",
            Self::UnsupportedDeclaration => "an exported declaration kind is not supported",
            Self::EmptyBindingPattern => "an exported destructuring pattern binds no name",
            Self::ShorthandAmbientModule => "a shorthand ambient module lists no names",
            Self::AnonymousDefaultWithoutSignature => {
                "an anonymous default function has no signature to export"
            }
            Self::NonObjectExportAssignment => "`export =` of a value that may carry properties",
            Self::UnknownLocalExport => "an export names something not declared in the file",
            Self::UnresolvedReExport => "a re-exported import has no known source",
            Self::DeclarationFileWithoutExportStatement => {
                "a declaration file with no export statement exports its unmarked names"
            }
            Self::UnrepresentableDeclaration => "an exported declaration has no portable form",
            Self::StarFromPartialModule => "`export *` from a module with a partial list",
            Self::NamespaceStarFromPartialModule => {
                "`export * as ns` from a module with a partial list"
            }
            Self::StarFromUnknownModule => "`export *` from a module that is missing or not read",
            Self::NamespaceStarFromUnknownModule => {
                "`export * as ns` from a module that is missing or not read"
            }
        }
    }
}

// Why a star re-export leaves the list partial, if it does. A module whose list was read and is
// complete adds nothing. One whose list was read and is partial passes its partiality on. Anything
// else (no file for the specifier, a file whose exports were not read, no import table at all) is
// a problem with the module itself and is reported as such, so a missing module is not counted as
// a partial list upstream.
fn star_reason(
    known: Option<ModuleExports>,
    partial: IncompleteReason,
    unknown: IncompleteReason,
) -> Option<IncompleteReason> {
    match known {
        Some(ModuleExports::Known { complete: true }) => None,
        Some(ModuleExports::Known { complete: false }) => Some(partial),
        Some(ModuleExports::Unresolved | ModuleExports::Unknown) | None => Some(unknown),
    }
}

fn note(reasons: &mut Vec<IncompleteReason>, reason: IncompleteReason) {
    if !reasons.contains(&reason) {
        reasons.push(reason);
    }
}

/// What `collect_exports` found: the declarations, the names they are exported under,
/// and whether anything exported had to be left out.
#[derive(Default)]
pub(crate) struct ExportTable {
    pub decls: Vec<PortableDecl>,
    /// Export name to the declaration's key. A name that is both a type and a value
    /// (`interface Foo` and `const Foo`) appears once for each; the declaration's kind
    /// tells them apart. A name that is an import exported again maps to a key whose
    /// declaration is in another unit's `decls`.
    pub names: Vec<(String, DeclKey)>,
    /// Why the list is partial; empty when it is complete. See `IncompleteReason`.
    pub incomplete_reasons: Vec<IncompleteReason>,
    /// True when the collector ran at all (the unit had a module key). A table that
    /// was never collected is not a complete list of nothing.
    pub collected: bool,
    /// The modules a script file declares with `declare module "x"`, by specifier.
    pub ambient: Vec<(String, ExportTable)>,
    /// `declare module "x"` declarations written in an external module. These augment
    /// the resolved module instead of answering the specifier themselves.
    pub augmentations: Vec<(String, ExportTable)>,
    /// UMD names introduced by `export as namespace`.
    pub umd_namespaces: Vec<String>,
}

impl ExportTable {
    pub(crate) fn is_incomplete(&self) -> bool {
        !self.incomplete_reasons.is_empty()
    }

    pub(crate) fn mark(&mut self, reason: IncompleteReason) {
        note(&mut self.incomplete_reasons, reason);
    }
}

// A top-level declaration the exports may be built from, and the names it is exported
// under (empty for one that is not exported).
struct Item<'ast> {
    name: String,
    kind: DeclKind,
    // The position among the earlier declarations of the same name and kind in the file,
    // counted over every top-level declaration, exported or not.
    index: u32,
    node: Node<'ast>,
    exported_as: Vec<String>,
    overloaded: bool,
}

enum Node<'ast> {
    Interface,
    Alias,
    Function(&'ast Function<'ast>),
    Var {
        symbol: Option<SymbolId>,
    },
    Enum {
        symbol: Option<SymbolId>,
    },
    // The instance side of a class: read from the namespace like an interface.
    Class,
    // The constructor, the class name used as a value.
    ClassValue {
        symbol: Option<SymbolId>,
    },
    // A namespace as a value: the object its exported members make.
    NamespaceValue {
        symbol: Option<SymbolId>,
    },
    /// An expression exported as the module's default value. Its type is inferred from
    /// the already-checked expression while exports are serialized.
    DefaultExpression(&'ast Expression<'ast>),
    DefaultFunction(&'ast Function<'ast>),
    DefaultClassInstance(&'ast Class<'ast>),
    DefaultClassValue(&'ast Class<'ast>),
    /// `export * as ns from "./m"`: the value side is a namespace object made from the
    /// source module's exported value declarations.
    ReExportNamespace {
        source: String,
    },
    /// A property type exposed through an `export =` object.
    Typed(TypeId),
}

// One export waiting for the resolver to be complete: every exported declaration has to
// have its slot and type parameters registered before any of them is written out,
// because they may name each other in any order.
struct Pending {
    key: DeclKey,
    name: String,
    kind: DeclKind,
    body: TypeId,
    type_params: Vec<TypeParamInput>,
    exported_as: Vec<String>,
}

// The unit's side of the contract (LLD 3.4): which slot is which declaration, and which
// type parameter belongs to which. Built from the exports only, so a body that names a
// declaration outside them gets no key and the export is reported as unsupported.
#[derive(Default)]
struct UnitKeys {
    slots: FxHashMap<DeclSlot, (DeclKey, String)>,
    tparams: FxHashMap<TypeParameterId, TypeParamKey>,
}

impl KeyResolver for UnitKeys {
    fn decl_key(&self, slot: DeclSlot) -> Option<DeclKey> {
        self.slots.get(&slot).map(|(key, _)| *key)
    }
    fn decl_name(&self, slot: DeclSlot) -> Option<String> {
        self.slots.get(&slot).map(|(_, name)| name.clone())
    }
    fn type_param_key(&self, id: TypeParameterId) -> Option<TypeParamKey> {
        self.tparams.get(&id).copied()
    }
}

// The unit's own declarations, then the ones it imported: an export may name a type that
// came from another file, and that type keeps the key it has in its own file.
struct WithImports<'a> {
    unit: &'a UnitKeys,
    imported: &'a Importer,
}

impl KeyResolver for WithImports<'_> {
    fn decl_key(&self, slot: DeclSlot) -> Option<DeclKey> {
        self.unit
            .decl_key(slot)
            .or_else(|| self.imported.decl_key(slot))
    }
    fn decl_name(&self, slot: DeclSlot) -> Option<String> {
        self.unit
            .decl_name(slot)
            .or_else(|| self.imported.decl_name(slot))
    }
    fn type_param_key(&self, id: TypeParameterId) -> Option<TypeParamKey> {
        self.unit
            .type_param_key(id)
            .or_else(|| self.imported.type_param_key(id))
    }
}

// `export { local as exported }`, before it is matched to a declaration.
struct Specifier {
    local: String,
    exported: String,
}

fn export_name(name: &ModuleExportName<'_>) -> String {
    match name {
        ModuleExportName::IdentifierName(n) => n.name.to_string(),
        ModuleExportName::IdentifierReference(n) => n.name.to_string(),
        ModuleExportName::StringLiteral(s) => s.value.to_string(),
    }
}

// Adds a declaration to the list and gives it its merge index.
fn push_item<'ast>(
    items: &mut Vec<Item<'ast>>,
    seen: &mut FxHashMap<(String, u8), u32>,
    name: &str,
    kind: DeclKind,
    node: Node<'ast>,
    exported_as: Vec<String>,
) {
    let slot = seen.entry((name.to_string(), kind as u8)).or_insert(0);
    let index = *slot;
    *slot += 1;
    items.push(Item {
        name: name.to_string(),
        kind,
        index,
        node,
        exported_as,
        overloaded: false,
    });
}

// Export every identifier introduced by a destructuring declaration. The checker
// already records the individual symbol types during the body pass; this walk only
// recovers their names and symbol IDs for the portable export table.
fn collect_binding_identifiers<'ast>(
    pattern: &'ast BindingPattern<'ast>,
    out: &mut Vec<(String, Option<SymbolId>)>,
) {
    match pattern {
        BindingPattern::BindingIdentifier(id) => {
            out.push((id.name.to_string(), id.symbol_id.get()))
        }
        BindingPattern::ObjectPattern(object) => {
            for property in &object.properties {
                collect_binding_identifiers(&property.value, out);
            }
            if let Some(rest) = &object.rest {
                collect_binding_identifiers(&rest.argument, out);
            }
        }
        BindingPattern::ArrayPattern(array) => {
            for element in array.elements.iter().flatten() {
                collect_binding_identifiers(element, out);
            }
            if let Some(rest) = &array.rest {
                collect_binding_identifiers(&rest.argument, out);
            }
        }
        BindingPattern::AssignmentPattern(assignment) => {
            collect_binding_identifiers(&assignment.left, out);
        }
    }
}

fn re_export_namespace(
    ctx: &mut CheckContext<'_, '_>,
    module: ModuleKey,
    item: &Item<'_>,
    source: &str,
) -> Option<Pending> {
    let imports = ctx.imports?;
    // A partial module still contributes the exports the checker knows about. The
    // caller marks the forwarding table incomplete, but preserving known members is
    // better than dropping the entire namespace value.
    if !matches!(imports.module_exports(source), ModuleExports::Known { .. }) {
        return None;
    }
    let provider: &dyn crate::portable::DeclProvider = imports;
    let mut properties = Vec::new();
    let mut seen_names = std::collections::HashSet::new();
    for (name, key) in imports.export_all(source) {
        let decl = provider.decl(key)?;
        if !matches!(
            decl.kind,
            DeclKind::Function | DeclKind::Var | DeclKind::Enum
        ) {
            continue;
        }
        if !seen_names.insert(name.clone()) {
            continue;
        }
        let value = match decl.kind {
            DeclKind::Function | DeclKind::Var => {
                ctx.importer.import_decl(&mut ctx.arena, provider, key)
            }
            DeclKind::Enum => ctx
                .importer
                .import_enum(&mut ctx.arena, provider, key)
                .map(|(_, value)| value),
            _ => None,
        }?;
        properties.push(PropertyEntry {
            name: name.into(),
            type_id: value,
            optional: false,
            is_method: false,
        });
    }
    let body = ctx.arena.alloc(Type::Object(ObjectType::new(properties)));
    Some(pending_of(module, item, body))
}

fn push_declaration<'ast>(
    items: &mut Vec<Item<'ast>>,
    seen: &mut FxHashMap<(String, u8), u32>,
    declaration: &'ast Declaration<'ast>,
    exported: bool,
    // The declaration is in an ambient context (a declaration file, or a `declare module`
    // block), where the members of a namespace are exported unless some are marked.
    ambient: bool,
    incomplete: &mut Vec<IncompleteReason>,
) {
    let named = |name: &str| {
        if exported {
            vec![name.to_string()]
        } else {
            Vec::new()
        }
    };
    match declaration {
        Declaration::TSInterfaceDeclaration(d) => {
            let names = named(&d.id.name);
            push_item(
                items,
                seen,
                &d.id.name,
                DeclKind::Interface,
                Node::Interface,
                names,
            );
        }
        Declaration::TSTypeAliasDeclaration(d) => {
            let names = named(&d.id.name);
            push_item(items, seen, &d.id.name, DeclKind::Alias, Node::Alias, names);
        }
        Declaration::FunctionDeclaration(f) => {
            if let Some(id) = &f.id {
                let names = named(&id.name);
                push_item(
                    items,
                    seen,
                    &id.name,
                    DeclKind::Function,
                    Node::Function(f),
                    names,
                );
            } else if exported {
                note(incomplete, IncompleteReason::UnsupportedDeclaration);
            }
        }
        Declaration::VariableDeclaration(v) => {
            for declarator in &v.declarations {
                let mut bindings = Vec::new();
                collect_binding_identifiers(&declarator.id, &mut bindings);
                if bindings.is_empty() && exported {
                    note(incomplete, IncompleteReason::EmptyBindingPattern);
                }
                for (name, symbol) in bindings {
                    let names = named(&name);
                    push_item(
                        items,
                        seen,
                        &name,
                        DeclKind::Var,
                        Node::Var { symbol },
                        names,
                    );
                }
            }
        }
        Declaration::TSEnumDeclaration(d) => {
            let names = named(&d.id.name);
            let node = Node::Enum {
                symbol: d.id.symbol_id.get(),
            };
            push_item(items, seen, &d.id.name, DeclKind::Enum, node, names);
        }
        Declaration::ClassDeclaration(c) => {
            if let Some(id) = &c.id {
                push_class(items, seen, &id.name, id.symbol_id.get(), named(&id.name));
            } else if exported {
                note(incomplete, IncompleteReason::UnsupportedDeclaration);
            }
        }
        Declaration::TSNamespaceDeclaration(ns) => {
            push_namespace(items, seen, ns, named(&ns.id.name), ambient)
        }
        // A module block has a value side the portable form does not carry yet. Not exported, they are nothing to us.
        _ => {
            if exported {
                note(incomplete, IncompleteReason::UnsupportedDeclaration);
            }
        }
    }
}

pub(crate) fn collect_exports<'ast>(
    program: &'ast Program<'ast>,
    scoping: &Scoping,
    ctx: &mut CheckContext<'ast, '_>,
    module: ModuleKey,
) -> ExportTable {
    let definition = program.source_type.is_typescript_definition();
    let mut table = collect_in(&program.body, definition, false, scoping, ctx, module);
    let (ambient, augmentations) = collect_ambient(program, scoping, ctx, module);
    table.ambient = ambient;
    table.augmentations = augmentations;
    table
}

// The modules a script file declares with `declare module "x" { ... }`: each is a module
// answering the specifier `x` for every file of the project, with the exports its block
// makes. In a file that is itself a module (it has an import or an export) the same
// syntax augments a module that exists elsewhere, which this does not read.
fn collect_ambient<'ast>(
    program: &'ast Program<'ast>,
    scoping: &Scoping,
    ctx: &mut CheckContext<'ast, '_>,
    owner: ModuleKey,
) -> (Vec<(String, ExportTable)>, Vec<(String, ExportTable)>) {
    let is_module = program.body.iter().any(|stmt| {
        matches!(
            stmt,
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
    let mut ambient = Vec::new();
    let mut augmentations = Vec::new();
    let mut ordinals: FxHashMap<String, u32> = FxHashMap::default();
    for stmt in &program.body {
        let Statement::TSExternalModuleDeclaration(module) = stmt else {
            continue;
        };
        let specifier = module.id.value.to_string();
        let ordinal = ordinals.entry(specifier.clone()).or_insert(0);
        let block_ordinal = *ordinal;
        *ordinal += 1;
        let module_key = ModuleKey::ambient_child(&specifier, owner, block_ordinal);
        // `declare module "x";` with no block says only that `x` exists, and that
        // everything imported from it is `any`. It answers the specifier, so the import is
        // not a missing module, and it lists no names, so no name is called missing.
        let Some(body) = &module.body else {
            let result = ExportTable {
                collected: true,
                incomplete_reasons: vec![IncompleteReason::ShorthandAmbientModule],
                ..ExportTable::default()
            };
            let destination = if is_module {
                &mut augmentations
            } else {
                &mut ambient
            };
            destination.push((specifier, result));
            continue;
        };
        // The block's declarations were registered inside this scope, so resolving a
        // name in it is what finds them.
        ctx.namespace
            .push_scope(&super::declare::module_scope(&specifier));
        // With no explicit `export` anywhere in the block, every member is exported.
        let marked = body.body.iter().any(|stmt| {
            matches!(
                stmt,
                Statement::ExportDeclaration(_)
                    | Statement::ExportNamedDeclaration(_)
                    | Statement::ExportDefaultDeclaration(_)
                    | Statement::ExportAllDeclaration(_)
                    | Statement::ExportFromDeclaration(_)
                    | Statement::TSExportAssignment(_)
            )
        });
        let table = collect_in(
            &body.body,
            true,
            // An augmentation contributes declarations to an existing module. Its
            // declaration members are part of the augmented namespace even when the
            // source does not repeat `export` on every declaration.
            is_module || !marked,
            scoping,
            ctx,
            module_key,
        );
        ctx.namespace.pop_scope();
        if is_module {
            augmentations.push((specifier, table));
        } else {
            ambient.push((specifier, table));
        }
    }
    (ambient, augmentations)
}

fn collect_in<'ast>(
    statements: &'ast [Statement<'ast>],
    definition: bool,
    // Every top-level declaration is exported, as in an ambient module block that marks
    // none of its members.
    export_all: bool,
    scoping: &Scoping,
    ctx: &mut CheckContext<'ast, '_>,
    module: ModuleKey,
) -> ExportTable {
    let mut table = ExportTable {
        collected: true,
        ..ExportTable::default()
    };
    let mut items: Vec<Item<'ast>> = Vec::new();
    let mut seen: FxHashMap<(String, u8), u32> = FxHashMap::default();
    let mut specifiers: Vec<Specifier> = Vec::new();
    // `export { a } from "./a"`: the source as written, and its specifiers.
    let mut from_modules: Vec<(String, Vec<Specifier>)> = Vec::new();
    // `export = value` carries the module's whole value. Object values also expose
    // their properties to named imports for common declaration-file interop.
    let mut export_assignment: Option<&'ast Expression<'ast>> = None;
    let mut export_assignment_type: Option<TypeId> = None;
    // `export * from "./a"`: the source as written.
    let mut stars: Vec<String> = Vec::new();
    // `export * as ns from "./a"` also carries type names under `ns.Name`, not just
    // the namespace object's value properties.
    let mut namespace_stars: Vec<(String, String)> = Vec::new();

    for stmt in statements {
        match stmt {
            Statement::ExportDeclaration(export) => push_declaration(
                &mut items,
                &mut seen,
                &export.declaration,
                true,
                definition,
                &mut table.incomplete_reasons,
            ),
            Statement::ExportDefaultDeclaration(export) => match &export.declaration {
                ExportDefaultDeclarationKind::FunctionDeclaration(f) if f.id.is_some() => {
                    let id = f.id.as_ref().map(|id| id.name.as_str()).unwrap_or_default();
                    let names = vec!["default".to_string()];
                    push_item(
                        &mut items,
                        &mut seen,
                        id,
                        DeclKind::Function,
                        Node::Function(f),
                        names,
                    );
                }
                ExportDefaultDeclarationKind::FunctionDeclaration(f) => {
                    // The body pass checks anonymous functions and records their complete
                    // FunctionType by source offset. In declaration files there is no body,
                    // so only an explicitly annotated signature can be carried.
                    if f.body.is_some() || f.return_type.is_some() {
                        push_item(
                            &mut items,
                            &mut seen,
                            "default",
                            DeclKind::Function,
                            Node::DefaultFunction(f),
                            vec!["default".to_string()],
                        );
                    } else {
                        table.mark(IncompleteReason::AnonymousDefaultWithoutSignature);
                    }
                }
                ExportDefaultDeclarationKind::TSInterfaceDeclaration(d) => {
                    let names = vec!["default".to_string()];
                    push_item(
                        &mut items,
                        &mut seen,
                        &d.id.name,
                        DeclKind::Interface,
                        Node::Interface,
                        names,
                    );
                }
                ExportDefaultDeclarationKind::ClassDeclaration(c) if c.id.is_some() => {
                    if let Some(id) = &c.id {
                        push_class(
                            &mut items,
                            &mut seen,
                            &id.name,
                            id.symbol_id.get(),
                            vec!["default".to_string()],
                        );
                    }
                }
                ExportDefaultDeclarationKind::ClassDeclaration(c) => {
                    // Anonymous default classes still have a portable instance shape and
                    // a constructor value. Use the reserved export name for both halves;
                    // neither is inserted into the source file's ordinary namespace.
                    push_item(
                        &mut items,
                        &mut seen,
                        "default",
                        DeclKind::Class,
                        Node::DefaultClassInstance(c),
                        vec!["default".to_string()],
                    );
                    push_item(
                        &mut items,
                        &mut seen,
                        "default",
                        DeclKind::Var,
                        Node::DefaultClassValue(c),
                        vec!["default".to_string()],
                    );
                }
                // `export default Foo` names a declaration of this file, so it is
                // `export { Foo as default }`.
                ExportDefaultDeclarationKind::Identifier(id) => specifiers.push(Specifier {
                    local: id.name.to_string(),
                    exported: "default".to_string(),
                }),
                other if other.as_expression().is_some() => {
                    if let Some(expr) = other.as_expression() {
                        push_item(
                            &mut items,
                            &mut seen,
                            "default",
                            DeclKind::Var,
                            Node::DefaultExpression(expr),
                            vec!["default".to_string()],
                        );
                    }
                }
                // Every default-export form oxc has today is handled above, so this arm is
                // not reached. It stays so that a form a later oxc adds leaves the list
                // partial instead of being skipped silently.
                _ => table.mark(IncompleteReason::UnsupportedDeclaration),
            },
            Statement::ExportNamedDeclaration(named) => {
                for spec in &named.specifiers {
                    specifiers.push(Specifier {
                        local: export_name(&spec.local),
                        exported: export_name(&spec.exported),
                    });
                }
            }
            Statement::ExportFromDeclaration(from) => {
                let list = from
                    .specifiers
                    .iter()
                    .map(|spec| Specifier {
                        local: export_name(&spec.local),
                        exported: export_name(&spec.exported),
                    })
                    .collect();
                from_modules.push((from.source.value.to_string(), list));
            }
            // `export * from "./a"` re-exports what ./a exports. `export * as ns`
            // additionally exports the source module's value side as an object.
            Statement::ExportAllDeclaration(all) => {
                if let Some(exported) = &all.exported {
                    let alias = export_name(exported);
                    let source = all.source.value.to_string();
                    push_item(
                        &mut items,
                        &mut seen,
                        &alias,
                        DeclKind::Var,
                        Node::ReExportNamespace {
                            source: source.clone(),
                        },
                        vec![alias.clone()],
                    );
                    namespace_stars.push((alias, source));
                } else {
                    stars.push(all.source.value.to_string());
                }
            }
            Statement::TSInterfaceDeclaration(d) => push_item(
                &mut items,
                &mut seen,
                &d.id.name,
                DeclKind::Interface,
                Node::Interface,
                Vec::new(),
            ),
            Statement::TSTypeAliasDeclaration(d) => push_item(
                &mut items,
                &mut seen,
                &d.id.name,
                DeclKind::Alias,
                Node::Alias,
                Vec::new(),
            ),
            Statement::ClassDeclaration(c) => {
                if let Some(id) = &c.id {
                    push_class(
                        &mut items,
                        &mut seen,
                        &id.name,
                        id.symbol_id.get(),
                        Vec::new(),
                    );
                }
            }
            // A namespace declaration can be named by an explicit export or can be
            // exported through an ambient declaration context.
            Statement::TSNamespaceDeclaration(ns) => {
                // Not exported by itself, but `export { N }` or an exported declaration
                // may still reach it, so it is registered with no names.
                push_namespace(&mut items, &mut seen, ns, Vec::new(), definition);
            }
            // `export = name` hands the module's whole value to one declaration. It is
            // listed under the name `export=`, which no source can write, so an
            // `import x = require()` or a default import can find it. Object properties
            // are collected below for named-import compatibility.
            Statement::TSExportAssignment(assign) => {
                export_assignment = Some(&assign.expression);
                if let Expression::Identifier(id) = &assign.expression {
                    // Preserve the declaration's own identity when the assignment names
                    // a local or imported declaration. It may carry generic parameters or
                    // references that would be lost by copying only its value shape.
                    let name = id.name.to_string();
                    specifiers.push(Specifier {
                        local: name,
                        exported: "export=".to_string(),
                    });
                } else {
                    // Expressions have no declaration slot of their own. They still have
                    // a real inferred type, which can be serialized as the module's
                    // `export=` value instead of making the entire table incomplete.
                    let diagnostic_count = ctx.diagnostics.len();
                    let value_type =
                        super::expressions::infer_expression_type(&assign.expression, scoping, ctx);
                    ctx.diagnostics.truncate(diagnostic_count);
                    export_assignment_type = Some(value_type);
                    push_item(
                        &mut items,
                        &mut seen,
                        "export=",
                        DeclKind::Var,
                        Node::Typed(value_type),
                        vec!["export=".to_string()],
                    );
                }
            }
            // `export as namespace X` declares a global UMD name. It does not add a
            // named export to this module's import table, so it does not make that table
            // incomplete (the global-script binding itself is handled separately).
            Statement::TSNamespaceExportDeclaration(namespace) => {
                let name = namespace.id.name.to_string();
                if !table
                    .umd_namespaces
                    .iter()
                    .any(|existing| existing == &name)
                {
                    table.umd_namespaces.push(name);
                }
            }
            Statement::TSEnumDeclaration(d) => {
                let node = Node::Enum {
                    symbol: d.id.symbol_id.get(),
                };
                push_item(
                    &mut items,
                    &mut seen,
                    &d.id.name,
                    DeclKind::Enum,
                    node,
                    Vec::new(),
                );
            }
            Statement::FunctionDeclaration(f) => {
                if let Some(id) = &f.id {
                    push_item(
                        &mut items,
                        &mut seen,
                        &id.name,
                        DeclKind::Function,
                        Node::Function(f),
                        Vec::new(),
                    );
                }
            }
            Statement::VariableDeclaration(v) => {
                for declarator in &v.declarations {
                    let mut bindings = Vec::new();
                    collect_binding_identifiers(&declarator.id, &mut bindings);
                    for (name, symbol) in bindings {
                        push_item(
                            &mut items,
                            &mut seen,
                            &name,
                            DeclKind::Var,
                            Node::Var { symbol },
                            Vec::new(),
                        );
                    }
                }
            }
            _ => {}
        }
    }

    // `export = value` permits named imports of properties from an object value in
    // common CommonJS declaration patterns. Give each property a portable declaration
    // keyed in this module, while retaining the original value under `export=`.
    if let Some(expression) = export_assignment {
        let value_type = export_assignment_type.or_else(|| {
            if let Expression::Identifier(id) = expression {
                id.reference_id
                    .get()
                    .and_then(|reference_id| scoping.get_reference(reference_id).symbol_id())
                    .and_then(|symbol| ctx.symbols.get(symbol))
                    .or_else(|| {
                        let symbol = items.iter().find_map(|item| {
                            if item.name != id.name.as_str() {
                                return None;
                            }
                            match &item.node {
                                Node::Var { symbol } | Node::ClassValue { symbol } => *symbol,
                                _ => None,
                            }
                        });
                        symbol.and_then(|symbol| ctx.symbols.get(symbol))
                    })
            } else {
                let diagnostic_count = ctx.diagnostics.len();
                let ty = super::expressions::infer_expression_type(expression, scoping, ctx);
                ctx.diagnostics.truncate(diagnostic_count);
                Some(ty)
            }
        });
        let properties = value_type.and_then(|ty| match ctx.arena.get(ty) {
            Type::Object(object) => Some(object.properties.clone()),
            _ => None,
        });
        if let Some(properties) = properties {
            for property in properties.iter() {
                if items.iter().any(|item| {
                    item.name == property.name.as_ref()
                        && item.kind == DeclKind::Var
                        && !item.exported_as.is_empty()
                }) {
                    continue;
                }
                push_item(
                    &mut items,
                    &mut seen,
                    &property.name,
                    DeclKind::Var,
                    Node::Typed(property.type_id),
                    vec![property.name.to_string()],
                );
            }
        } else {
            // A non-object `export =` value is preserved under the assignment key, but
            // this portable model cannot prove that it has no named/static properties
            // (functions can carry properties too). Keep the module partial rather than
            // incorrectly diagnosing every named import as definitely absent.
            table.mark(IncompleteReason::NonObjectExportAssignment);
        }
    }

    // Overloads are separate public signatures; an implementation body is not part of
    // the callable API. Keep each signature as its own stable declaration key, then the
    // importer combines them into a callable intersection.
    for item in &mut items {
        if item.kind == DeclKind::Function
            && seen
                .get(&(item.name.clone(), item.kind as u8))
                .copied()
                .unwrap_or(0)
                > 1
        {
            item.overloaded = true;
        }
    }
    // The implementation contributes behavior, not a public signature. Remove it before
    // resolving `export { pick }`, otherwise the specifier would accidentally re-export it.
    items.retain(|item| {
        !matches!(&item.node, Node::Function(function) if item.overloaded && function.body.is_some())
    });

    // `export { a, b as c }`: each names a declaration of this file (any kind that has
    // that name, so a type and a value both go out), or something this file imported.
    for spec in specifiers {
        let mut found = false;
        let qualified = format!("{}.", spec.local);
        for item in items.iter_mut() {
            if item.name == spec.local {
                item.exported_as.push(spec.exported.clone());
                found = true;
            } else if let Some(rest) = item.name.strip_prefix(&qualified) {
                // The types inside a namespace go out under the namespace's new name.
                item.exported_as.push(format!("{}.{rest}", spec.exported));
            }
        }
        if found {
            continue;
        }
        match ctx.imported_names.get(&spec.local) {
            Some(keys) if !keys.is_empty() => {
                for key in keys {
                    table.names.push((spec.exported.clone(), *key));
                }
                let qualified = format!("{}.", spec.local);
                let mut inner: Vec<(&String, &Vec<DeclKey>)> = ctx
                    .imported_names
                    .iter()
                    .filter(|(name, _)| name.starts_with(&qualified))
                    .collect();
                inner.sort_by(|a, b| a.0.cmp(b.0));
                for (name, keys) in inner {
                    for key in keys {
                        table.names.push((
                            format!("{}.{}", spec.exported, &name[qualified.len()..]),
                            *key,
                        ));
                    }
                }
            }
            // An unknown local name is an error tsc reports; there is nothing to export.
            _ => table.mark(IncompleteReason::UnknownLocalExport),
        }
    }
    // `export { a } from "./a"`: the other module's own export, under its own key.
    for (source, list) in from_modules {
        for spec in list {
            let keys = ctx
                .imports
                .map(|imports| imports.export_keys(&source, &spec.local))
                .unwrap_or_default();
            if keys.is_empty() {
                table.mark(IncompleteReason::UnresolvedReExport);
            }
            for key in keys {
                table.names.push((spec.exported.clone(), key));
            }
            // `export { N } from` also carries the types written inside N.
            let qualified = format!("{}.", spec.local);
            for (name, key) in ctx
                .imports
                .map(|imports| imports.export_all(&source))
                .unwrap_or_default()
            {
                if let Some(rest) = name.strip_prefix(&qualified) {
                    table.names.push((format!("{}.{rest}", spec.exported), key));
                }
            }
        }
    }

    let mut keys = UnitKeys::default();
    let mut pending: Vec<Pending> = Vec::new();
    if export_all {
        for item in items.iter_mut() {
            if item.exported_as.is_empty() {
                item.exported_as.push(item.name.clone());
            }
        }
    }

    // Problem: in a declaration file a top-level declaration with no `export` is still
    // exported, unless the file has an export statement (`export {}`, `export { a }`,
    // `export * from`, `export =`, `export default x`), which turns that off. Treating every
    // unmarked declaration as private reported a valid import as a missing member.
    // Picked: the list is complete when the file has such a statement, and partial when it
    // does not, because then which declarations are exported cannot be told from here. Checked
    // against tsc 6.0.3: with `export {}` a local name is TS2459, without one it imports.
    // Cost: a declaration file with no export statement keeps a partial list, so a misspelled
    // name in an import from it is not reported.
    let has_export_statement = statements.iter().any(|stmt| match stmt {
        Statement::ExportNamedDeclaration(_)
        | Statement::ExportFromDeclaration(_)
        | Statement::ExportAllDeclaration(_)
        | Statement::TSExportAssignment(_) => true,
        Statement::ExportDefaultDeclaration(export) => export.declaration.as_expression().is_some(),
        _ => false,
    });
    if definition && !has_export_statement && items.iter().any(|item| item.exported_as.is_empty()) {
        table.mark(IncompleteReason::DeclarationFileWithoutExportStatement);
    }

    // Every interface and alias of the file is registered, exported or not: an exported
    // declaration may name one that is not (`interface Hidden` and `export function
    // f(h: Hidden)`), and it can only be written out with a key if the resolver knows it.
    // A declaration that cannot be built is skipped when it is not exported, and marks
    // the table incomplete when it is.
    for item in &items {
        let exported = !item.exported_as.is_empty();
        if !exported
            && !matches!(
                item.kind,
                DeclKind::Interface | DeclKind::Alias | DeclKind::Class
            )
        {
            continue;
        }
        let built = match &item.node {
            Node::Interface | Node::Alias => interface_or_alias(ctx, module, item),
            Node::Function(f) => function(ctx, module, f, item),
            Node::DefaultFunction(f) => default_function(ctx, module, f, item),
            Node::DefaultClassInstance(class) => default_class_instance(ctx, module, class, item),
            Node::DefaultClassValue(class) => default_class_value(ctx, module, class, item),
            Node::Var { symbol } => variable(ctx, module, *symbol, item),
            Node::Enum { symbol } => enumeration(ctx, module, *symbol, item),
            Node::Class => interface_or_alias(ctx, module, item),
            Node::ClassValue { symbol } => variable(ctx, module, *symbol, item),
            Node::NamespaceValue { symbol } => variable(ctx, module, *symbol, item),
            Node::DefaultExpression(expr) => {
                // Expression checking has already happened in the body pass. Reuse the
                // same inference entry point to capture the result, but do not let the
                // export-serialization pass duplicate diagnostics.
                let diagnostic_count = ctx.diagnostics.len();
                let body = super::expressions::infer_expression_type(expr, scoping, ctx);
                ctx.diagnostics.truncate(diagnostic_count);
                Some(pending_of(module, item, body))
            }
            Node::ReExportNamespace { source } => re_export_namespace(ctx, module, item, source),
            Node::Typed(body) => Some(pending_of(module, item, *body)),
        };
        match built.and_then(|p| register(&mut keys, ctx, p)) {
            Some(p) => pending.push(p),
            None if exported => table.mark(IncompleteReason::UnrepresentableDeclaration),
            None => {}
        }
    }

    // Builtins (`Promise<T>`, `Record<K, V>`) are the one kind of declaration a body may
    // name without it being exported: they resolve to the built-in key.
    for name in BUILTIN_GENERICS {
        if let Some(slot) = ctx.arena.existing_builtin_slot(name) {
            keys.slots
                .insert(slot, (DeclKey::builtin(name), name.to_string()));
        }
    }

    let resolver = WithImports {
        unit: &keys,
        imported: &ctx.importer,
    };
    // Every candidate is written out; which of them leave the unit is decided below.
    let mut written: FxHashMap<DeclKey, (PortableDecl, Vec<String>)> = FxHashMap::default();
    let mut order: Vec<DeclKey> = Vec::new();
    for p in pending {
        let input = DeclInput {
            key: p.key,
            name: &p.name,
            kind: p.kind,
            body: p.body,
            type_params: &p.type_params,
        };
        match export_decl(&ctx.arena, &resolver, &input) {
            Some(decl) => {
                order.push(p.key);
                written.insert(p.key, (decl, p.exported_as));
            }
            None if !p.exported_as.is_empty() => {
                table.mark(IncompleteReason::UnrepresentableDeclaration)
            }
            None => {}
        }
    }

    // The exported declarations leave under their names. A declaration they reach but do
    // not export leaves under its own key and no name, so a dependent can follow the
    // reference without being able to import it. Reached ones are found by walking the
    // references of what already left, to a fixed point.
    let mut leaving: Vec<DeclKey> = order
        .iter()
        .copied()
        .filter(|key| !written[key].1.is_empty())
        .collect();
    let mut seen: std::collections::HashSet<DeclKey> = leaving.iter().copied().collect();
    let mut cursor = 0;
    while cursor < leaving.len() {
        let key = leaving[cursor];
        cursor += 1;
        for node in written[&key].0.body.nodes.iter() {
            if let PNode::DeclRef { key: reached, .. } | PNode::App { key: reached, .. } = node {
                if written.contains_key(reached) && seen.insert(*reached) {
                    leaving.push(*reached);
                }
            }
        }
    }
    // A name the file exports itself wins over one a star re-export brings in, and the
    // first star wins over a later one. The re-exported declarations are another unit's,
    // found through the provider under the key they already have. A star over a module
    // whose own list is partial, or that is not known, is partial too.
    for source in stars {
        let imports = ctx.imports;
        let known = imports.map(|i| i.module_exports(&source));
        if let Some(reason) = star_reason(
            known,
            IncompleteReason::StarFromPartialModule,
            IncompleteReason::StarFromUnknownModule,
        ) {
            table.mark(reason);
        }
        let local: std::collections::HashSet<String> = order
            .iter()
            .flat_map(|key| written[key].1.iter().cloned())
            .chain(table.names.iter().map(|(name, _)| name.clone()))
            .collect();
        for (name, key) in imports.map(|i| i.export_all(&source)).unwrap_or_default() {
            if !local.contains(&name) {
                table.names.push((name, key));
            }
        }
    }
    for key in order {
        if !seen.contains(&key) {
            continue;
        }
        if let Some((decl, names)) = written.remove(&key) {
            for name in names {
                table.names.push((name, key));
            }
            table.decls.push(decl);
        }
    }
    // A namespace re-export has both a value side (`ns.member`) and a type namespace
    // (`ns.Type`). Preserve the source declarations' stable keys rather than copying
    // their shapes into declarations owned by the barrel file.
    for (alias, source) in namespace_stars {
        let known = ctx.imports.map(|imports| imports.module_exports(&source));
        if let Some(reason) = star_reason(
            known,
            IncompleteReason::NamespaceStarFromPartialModule,
            IncompleteReason::NamespaceStarFromUnknownModule,
        ) {
            table.mark(reason);
        }
        let existing_names: std::collections::HashSet<String> =
            table.names.iter().map(|(name, _)| name.clone()).collect();
        for (name, key) in ctx
            .imports
            .map(|imports| imports.export_all(&source))
            .unwrap_or_default()
        {
            let qualified = format!("{alias}.{name}");
            if !existing_names.contains(&qualified) {
                table.names.push((qualified, key));
            }
        }
    }
    table
}

// An interface or an alias, read out of the namespace. The type id is the one every use
// of the name resolves to, so its slot is the slot the resolver has to know.
fn interface_or_alias(
    ctx: &mut CheckContext<'_, '_>,
    module: ModuleKey,
    item: &Item<'_>,
) -> Option<Pending> {
    // A second part of a merged interface is already inside the first's body, and a
    // redeclared alias is an error reported elsewhere.
    if item.index > 0 {
        return None;
    }
    let Resolution::Resolved(type_id) = ctx.namespace.resolve(&item.name, &mut ctx.arena) else {
        return None;
    };
    // An interface, a class and a type-literal alias come back as a Ref, a named alias
    // as a Named; a generic alias or a primitive alias is the body itself and nothing
    // refers to it by slot.
    let body = ctx
        .arena
        .decl_parts(type_id)
        .map_or(type_id, |(_, body)| body);
    Some(pending_of(module, item, body))
}

fn function<'ast>(
    ctx: &mut CheckContext<'ast, '_>,
    module: ModuleKey,
    f: &'ast Function<'ast>,
    item: &Item<'_>,
) -> Option<Pending> {
    if item.overloaded {
        let body = super::declare::resolve_function_type(f, ctx)?;
        return with_type_params(
            ctx,
            pending_of(module, item, body),
            f.type_parameters.as_deref(),
        );
    }
    let id = f.id.as_ref()?;
    let body = ctx.symbols.get(id.symbol_id.get()?)?;
    with_type_params(
        ctx,
        pending_of(module, item, body),
        f.type_parameters.as_deref(),
    )
}

fn default_function<'ast>(
    ctx: &mut CheckContext<'ast, '_>,
    module: ModuleKey,
    function: &'ast Function<'ast>,
    item: &Item<'_>,
) -> Option<Pending> {
    let body = ctx
        .exported_function_types
        .get(&function.span().start)
        .copied()
        .or_else(|| {
            function
                .id
                .as_ref()
                .and_then(|id| id.symbol_id.get())
                .and_then(|symbol| ctx.symbols.get(symbol))
        })
        .or_else(|| super::declare::resolve_function_type(function, ctx))?;
    with_type_params(
        ctx,
        pending_of(module, item, body),
        function.type_parameters.as_deref(),
    )
}

fn default_class_instance<'ast>(
    ctx: &mut CheckContext<'ast, '_>,
    module: ModuleKey,
    class: &'ast Class<'ast>,
    item: &Item<'_>,
) -> Option<Pending> {
    let body = ctx
        .namespace
        .resolve_anonymous_class(class, &mut ctx.arena)?;
    with_type_params(
        ctx,
        pending_of(module, item, body),
        class.type_parameters.as_deref(),
    )
}

fn default_class_value<'ast>(
    ctx: &mut CheckContext<'ast, '_>,
    module: ModuleKey,
    class: &'ast Class<'ast>,
    item: &Item<'_>,
) -> Option<Pending> {
    let instance = ctx
        .namespace
        .resolve_anonymous_class(class, &mut ctx.arena)?;
    let scope = ctx
        .namespace
        .push_decl_type_params(&mut ctx.arena, class.type_parameters.as_deref());
    let (params, is_untyped) = match super::declare::find_constructor(class) {
        Some(constructor) => match super::super::type_annotation::resolve_function_params(
            &constructor.params,
            &mut ctx.namespace,
            &mut ctx.arena,
        ) {
            Some(params) => (params, false),
            None => (Vec::new(), true),
        },
        None => (Vec::new(), false),
    };
    ctx.namespace.pop_type_params(scope);
    let body = ctx.arena.alloc(Type::Function(crate::types::FunctionType {
        params,
        return_type: instance,
        type_params: Vec::new(),
        is_untyped,
    }));
    with_type_params(
        ctx,
        pending_of(module, item, body),
        class.type_parameters.as_deref(),
    )
}

fn variable(
    ctx: &mut CheckContext<'_, '_>,
    module: ModuleKey,
    symbol: Option<SymbolId>,
    item: &Item<'_>,
) -> Option<Pending> {
    // The type is whatever the symbol holds once the bodies are checked: the annotation
    // when there is one, otherwise what the initializer inferred (a `const` keeps its
    // literal, a `let` is widened), which is the type tsc writes into a declaration file.
    if item.index > 0 {
        return None;
    }
    let body = ctx.symbols.get(symbol?)?;
    Some(pending_of(module, item, body))
}

// An enum is exported by its value-position type: an object of the member literals, which
// is all the importer needs to rebuild both the type and the value. A merged enum, or one
// the declare pass could not resolve (a computed member), has no such type here and is
// left out.
fn enumeration(
    ctx: &mut CheckContext<'_, '_>,
    module: ModuleKey,
    symbol: Option<SymbolId>,
    item: &Item<'_>,
) -> Option<Pending> {
    if item.index > 0 {
        return None;
    }
    let body = ctx.symbols.get(symbol?)?;
    Some(pending_of(module, item, body))
}

// A namespace is its value (an object of its exported members, exported under its own
// name) plus the types written inside it, each exported under its dotted name (`N.I`) so
// that `N.I` resolves in a file that imports `N`. Only members visible from outside go:
// the marked ones, or all of them in an ambient namespace where none is marked. A
// namespace this checker does not read (a dotted name) is reported as not pushed.
fn push_namespace<'ast>(
    items: &mut Vec<Item<'ast>>,
    seen: &mut FxHashMap<(String, u8), u32>,
    ns: &'ast oxc_ast::ast::TSNamespaceDeclaration<'ast>,
    names: Vec<String>,
    ambient: bool,
) {
    let node = Node::NamespaceValue {
        symbol: ns.id.symbol_id.get(),
    };
    push_item(items, seen, &ns.id.name, DeclKind::Var, node, names.clone());
    push_namespace_types(items, seen, ns, &ns.id.name, &names, ambient || ns.declare);
}

fn push_namespace_types<'ast>(
    items: &mut Vec<Item<'ast>>,
    seen: &mut FxHashMap<(String, u8), u32>,
    ns: &'ast oxc_ast::ast::TSNamespaceDeclaration<'ast>,
    prefix: &str,
    outer_names: &[String],
    ambient: bool,
) {
    use super::declare::TopLevelItem;
    let members = super::declare::namespace_members(ns);
    let any_marked = members.iter().any(|(_, marked)| *marked);
    for (item, marked) in members {
        if !(marked || (ambient && !any_marked)) {
            continue;
        }
        // Each type goes out under `prefix.Name`, once per name the namespace goes out
        // under, so `export { N as M }` is handled by the specifier pass instead.
        let dotted = |name: &str| format!("{prefix}.{name}");
        let names = |qualified: &str| -> Vec<String> {
            outer_names
                .iter()
                .map(|_| qualified.to_string())
                .take(1)
                .collect()
        };
        match item {
            TopLevelItem::Interface(d) => {
                let q = dotted(&d.id.name);
                push_item(
                    items,
                    seen,
                    &q,
                    DeclKind::Interface,
                    Node::Interface,
                    names(&q),
                );
            }
            TopLevelItem::Alias(d) => {
                let q = dotted(&d.id.name);
                push_item(items, seen, &q, DeclKind::Alias, Node::Alias, names(&q));
            }
            TopLevelItem::Class(c) => {
                if let Some(id) = &c.id {
                    let q = dotted(&id.name);
                    push_item(items, seen, &q, DeclKind::Class, Node::Class, names(&q));
                }
            }
            TopLevelItem::Enum(e) => {
                let q = dotted(&e.id.name);
                let node = Node::Enum {
                    symbol: e.id.symbol_id.get(),
                };
                push_item(items, seen, &q, DeclKind::Enum, node, names(&q));
            }
            TopLevelItem::Namespace(inner) => {
                let q = dotted(&inner.id.name);
                push_namespace_types(
                    items,
                    seen,
                    inner,
                    &q,
                    outer_names,
                    ambient || inner.declare,
                );
            }
            TopLevelItem::Function(_)
            | TopLevelItem::Variable(_)
            | TopLevelItem::ExternalModule(_) => {}
        }
    }
}

// A class is two declarations that share its name: the instance type, which another type
// can name, and the constructor, which is the name used as a value. Each is exported under
// the same names, and an import of the name binds both.
fn push_class<'ast>(
    items: &mut Vec<Item<'ast>>,
    seen: &mut FxHashMap<(String, u8), u32>,
    name: &str,
    symbol: Option<SymbolId>,
    names: Vec<String>,
) {
    push_item(
        items,
        seen,
        name,
        DeclKind::Class,
        Node::Class,
        names.clone(),
    );
    push_item(
        items,
        seen,
        name,
        DeclKind::Var,
        Node::ClassValue { symbol },
        names,
    );
}

fn pending_of(module: ModuleKey, item: &Item<'_>, body: TypeId) -> Pending {
    Pending {
        key: DeclKey::new(module, &item.name, item.kind, item.index),
        name: item.name.clone(),
        kind: item.kind,
        body,
        type_params: Vec::new(),
        exported_as: item.exported_as.clone(),
    }
}

// Registers an export with the resolver. An interface's type parameters may sit on a
// merged part and not on the exported one, so for an interface or an alias the namespace
// says which declaration actually carries them. None when the declaration cannot be read
// completely, which the caller reports as an unsupported export.
fn register<'ast>(
    keys: &mut UnitKeys,
    ctx: &mut CheckContext<'ast, '_>,
    mut p: Pending,
) -> Option<Pending> {
    if matches!(
        p.kind,
        DeclKind::Interface | DeclKind::Alias | DeclKind::Class
    ) {
        let declared = ctx.namespace.declared_type_param_decl(&p.name);
        p = with_type_params(ctx, p, declared)?;
        // The slot the rest of the unit knows this declaration by.
        if let Resolution::Resolved(type_id) = ctx.namespace.resolve(&p.name, &mut ctx.arena) {
            if let Some((slot, _)) = ctx.arena.decl_parts(type_id) {
                keys.slots.insert(slot, (p.key, p.name.clone()));
            }
        }
    }
    for (i, tp) in p.type_params.iter().enumerate() {
        keys.tparams.insert(
            tp.id,
            TypeParamKey {
                owner: p.key,
                index: i as u32,
            },
        );
    }
    // Generic function types can appear inside parameters, returns, object members,
    // aliases, and constraints. They have no top-level export declaration of their own,
    // so give their type parameters stable child keys before hashing or serializing the
    // body. Top-level signature parameters above keep the declaration key itself. The key
    // is the declaration's own key plus the path down to the signature, never a position
    // in the file.
    let parent = p.key;
    let mut path = Vec::new();
    register_nested_signature_keys(keys, &ctx.arena, &ctx.importer, parent, p.body, &mut path);
    for (index, parameter) in p.type_params.iter().enumerate() {
        for (tag, root) in [
            (TAG_DECL_BOUND, parameter.bound),
            (TAG_DECL_DEFAULT, parameter.default),
        ] {
            if let Some(root) = root {
                path.clear();
                path.push(tag);
                path.extend((index as u32).to_le_bytes());
                register_nested_signature_keys(
                    keys,
                    &ctx.arena,
                    &ctx.importer,
                    parent,
                    root,
                    &mut path,
                );
            }
        }
    }
    Some(p)
}

// One byte per step from a declaration's type down to a nested type, with a number where
// the step has an index, so two signatures in one declaration never share a path.
const TAG_PROPERTY: u8 = 1;
const TAG_PARAMETER: u8 = 2;
const TAG_RETURN: u8 = 3;
const TAG_MEMBER: u8 = 4;
const TAG_ELEMENT: u8 = 5;
const TAG_BOUND: u8 = 6;
const TAG_DEFAULT: u8 = 7;
const TAG_ARGUMENT: u8 = 8;
const TAG_INNER: u8 = 9;
const TAG_GENERIC_BOUND: u8 = 10;
const TAG_DECL_BOUND: u8 = 11;
const TAG_DECL_DEFAULT: u8 = 12;

fn register_nested_signature_keys(
    keys: &mut UnitKeys,
    arena: &crate::arena::TypeArena,
    imported: &Importer,
    parent: DeclKey,
    root: TypeId,
    path: &mut Vec<u8>,
) {
    struct Walk<'a> {
        keys: &'a mut UnitKeys,
        arena: &'a crate::arena::TypeArena,
        imported: &'a Importer,
        parent: DeclKey,
        // Membership only; it is never iterated, so its order cannot reach the output.
        seen: std::collections::HashSet<TypeId>,
    }

    // Runs `body` one step further down, and puts the path back afterwards.
    fn step(path: &mut Vec<u8>, tag: u8, index: Option<u32>, body: impl FnOnce(&mut Vec<u8>)) {
        let length = path.len();
        path.push(tag);
        if let Some(index) = index {
            path.extend(index.to_le_bytes());
        }
        body(path);
        path.truncate(length);
    }

    impl Walk<'_> {
        fn visit(&mut self, id: TypeId, path: &mut Vec<u8>) {
            if !self.seen.insert(id) {
                return;
            }
            match self.arena.raw(id) {
                Type::Function(function) => {
                    if !function.type_params.is_empty() {
                        let owner = DeclKey::nested_type_signature(self.parent, path);
                        for (index, parameter) in function.type_params.iter().enumerate() {
                            if self.keys.tparams.contains_key(&parameter.id)
                                || self.imported.type_param_key(parameter.id).is_some()
                            {
                                continue;
                            }
                            self.keys.tparams.insert(
                                parameter.id,
                                TypeParamKey {
                                    owner,
                                    index: index as u32,
                                },
                            );
                        }
                    }
                    for (index, parameter) in function.type_params.iter().enumerate() {
                        let index = index as u32;
                        if let Some(constraint) = parameter.constraint {
                            step(path, TAG_BOUND, Some(index), |p| self.visit(constraint, p));
                        }
                        if let Some(default) = parameter.default {
                            step(path, TAG_DEFAULT, Some(index), |p| self.visit(default, p));
                        }
                    }
                    for (index, parameter) in function.params.iter().enumerate() {
                        step(path, TAG_PARAMETER, Some(index as u32), |p| {
                            self.visit(parameter.type_id, p)
                        });
                    }
                    step(path, TAG_RETURN, None, |p| {
                        self.visit(function.return_type, p)
                    });
                }
                Type::Object(object) => {
                    for property in object.properties.iter() {
                        // The name is the step: properties are sorted by name, and a name
                        // does not move when the declaration does.
                        let length = path.len();
                        path.push(TAG_PROPERTY);
                        path.extend((property.name.len() as u32).to_le_bytes());
                        path.extend(property.name.as_bytes());
                        self.visit(property.type_id, path);
                        path.truncate(length);
                    }
                }
                Type::Array(element) => step(path, TAG_ELEMENT, None, |p| self.visit(*element, p)),
                Type::Union(members) | Type::Intersection(members) => {
                    for (index, &member) in members.iter().enumerate() {
                        step(path, TAG_MEMBER, Some(index as u32), |p| {
                            self.visit(member, p)
                        });
                    }
                }
                Type::GenericParameter(_, _, bound) => {
                    if let Some(bound) = bound {
                        step(path, TAG_GENERIC_BOUND, None, |p| self.visit(*bound, p));
                    }
                }
                Type::Named(_, inner) => step(path, TAG_INNER, None, |p| self.visit(*inner, p)),
                // An App's body belongs to its declaration, which is registered separately;
                // the application only introduces its own arguments at this use site.
                Type::App(_, arguments, _body) => {
                    for (index, &argument) in arguments.iter().enumerate() {
                        step(path, TAG_ARGUMENT, Some(index as u32), |p| {
                            self.visit(argument, p)
                        });
                    }
                }
                // A Ref intentionally stops at a declaration boundary. Its body receives
                // keys while its own declaration is registered.
                Type::Number
                | Type::String
                | Type::Boolean
                | Type::Null
                | Type::Undefined
                | Type::Any
                | Type::Unknown
                | Type::Error
                | Type::Never
                | Type::Void
                | Type::StringLiteral(_)
                | Type::NumberLiteral(_)
                | Type::BooleanLiteral(_)
                | Type::Ref(_) => {}
            }
        }
    }

    let mut walk = Walk {
        keys,
        arena,
        imported,
        parent,
        seen: std::collections::HashSet::new(),
    };
    walk.visit(root, path);
}

// The declaration's own `<T extends B = D, ...>` list as export inputs. The bound is read
// off the GenericParameter node the declare pass already allocated for the parameter, so
// it is the very type the body was resolved against. The default is resolved again with
// the declaration's parameters in scope (the namespace memoizes the parameter nodes, so
// a default that names an earlier parameter gets that same node). None when a parameter
// has no node, which means the declaration was never resolved with it in scope.
fn with_type_params<'ast>(
    ctx: &mut CheckContext<'ast, '_>,
    mut p: Pending,
    declared: Option<&'ast TSTypeParameterDeclaration<'ast>>,
) -> Option<Pending> {
    let Some(declared) = declared else {
        return Some(p);
    };
    let file_id = ctx.namespace.file_id();
    for (i, param) in declared.params.iter().enumerate() {
        let id = TypeParameterId::with_file(file_id, param.span().start, i as u32);
        let node = ctx.namespace.type_param_node(id)?;
        let Type::GenericParameter(_, _, bound) = ctx.arena.raw(node) else {
            return None;
        };
        let bound = *bound;
        let default = if param.default.is_some() {
            // A default that cannot be resolved cannot be exported.
            Some(
                ctx.namespace
                    .resolve_type_param_default(&mut ctx.arena, declared, i)?,
            )
        } else {
            None
        };
        p.type_params.push(TypeParamInput {
            id,
            name: param.name.name.to_string(),
            bound,
            default,
        });
    }
    Some(p)
}
