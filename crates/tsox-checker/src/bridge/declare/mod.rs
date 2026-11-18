use oxc_ast::ast::{
    BinaryOperator, BindingPattern, Class, Declaration, ExportDefaultDeclarationKind, Expression,
    Function, Program, Statement, TSEnumDeclaration, TSInterfaceDeclaration,
    TSNamespaceDeclaration, TSNamespaceDeclarationBody, TSTypeAliasDeclaration, UnaryOperator,
    VariableDeclaration,
};

use crate::type_annotation::{
    resolve_function_params, resolve_function_type_params, resolve_type_annotation,
};
use crate::types::Type;

use super::context::CheckContext;

// One declaration the declare pass cares about, whether it stands alone or sits
// behind `export` / `export default`.
//
// Problem: both passes matched on Statement::FunctionDeclaration and friends, and an
// exported declaration is a different statement kind that wraps the same payload, so
// `export function f(...)` and `export interface I {...}` were never declared at all
// (and the check pass then warned they were unsupported).
// Picked: look through the export wrapper once, here, and hand both passes the same
// flat list, so a declaration is treated the same whether or not it is exported.
// Cost: a list of references is built per pass; `export { a, b }` and
// `export * from` declare nothing and are skipped, which is correct for one file.
#[derive(Clone, Copy)]
pub(super) enum TopLevelItem<'a> {
    Alias(&'a TSTypeAliasDeclaration<'a>),
    Interface(&'a TSInterfaceDeclaration<'a>),
    Class(&'a Class<'a>),
    Enum(&'a TSEnumDeclaration<'a>),
    Variable(&'a VariableDeclaration<'a>),
    Function(&'a Function<'a>),
    Namespace(&'a TSNamespaceDeclaration<'a>),
    // `declare module "x" { ... }`: its members are declared in a scope named for `x`.
    ExternalModule(&'a oxc_ast::ast::TSExternalModuleDeclaration<'a>),
}

// The scope name of `declare module "x"`: quoted, so no identifier can collide with it.
pub(super) fn module_scope(specifier: &str) -> String {
    format!("\"{specifier}\"")
}

// The names declared at the top of `statements`, as `declare global { }` members or the
// top level of a script declare them. A `declare module "x"` block has no name here.
pub(super) fn declared_names<'ast>(statements: &'ast [Statement<'ast>]) -> Vec<String> {
    let mut names = Vec::new();
    for (item, _) in items_in(statements) {
        match item {
            TopLevelItem::Alias(d) => names.push(d.id.name.to_string()),
            TopLevelItem::Interface(d) => names.push(d.id.name.to_string()),
            TopLevelItem::Class(d) => {
                if let Some(id) = &d.id {
                    names.push(id.name.to_string());
                }
            }
            TopLevelItem::Enum(d) => names.push(d.id.name.to_string()),
            TopLevelItem::Function(d) => {
                if let Some(id) = &d.id {
                    names.push(id.name.to_string());
                }
            }
            TopLevelItem::Namespace(d) => names.push(d.id.name.to_string()),
            TopLevelItem::Variable(d) => {
                for declarator in &d.declarations {
                    let mut bindings = Vec::new();
                    super::exports::collect_binding_identifiers(&declarator.id, &mut bindings);
                    names.extend(bindings.into_iter().map(|(name, _)| name));
                }
            }
            TopLevelItem::ExternalModule(_) => {}
        }
    }
    names
}

// The names this file declares itself inside `declare global { ... }` blocks. Those are
// global to the project but are declared in this file, so the project's copy of one must not
// be bound here as well: it would collide with the local declaration.
pub(super) fn own_global_block_names(program: &Program<'_>) -> std::collections::HashSet<String> {
    let mut names = std::collections::HashSet::new();
    for stmt in &program.body {
        if let Statement::TSGlobalDeclaration(block) = stmt {
            names.extend(declared_names(&block.body.body));
        }
    }
    names
}

// The interfaces this file declares globally: those at the top of a script file and those
// inside its `declare global { }` blocks. Other files may declare the same name, and the
// declarations are one interface, so the project's copies are bound before this file's own.
pub(super) fn own_global_interface_names(program: &Program<'_>) -> Vec<String> {
    let mut names = Vec::new();
    let is_module = program.body.iter().any(|statement| {
        matches!(
            statement,
            Statement::ImportDeclaration(_)
                | Statement::ExportNamedDeclaration(_)
                | Statement::ExportDefaultDeclaration(_)
                | Statement::ExportAllDeclaration(_)
                | Statement::TSExportAssignment(_)
                | Statement::TSImportEqualsDeclaration(_)
        )
    });
    for stmt in &program.body {
        match stmt {
            Statement::TSInterfaceDeclaration(d) if !is_module => {
                names.push(d.id.name.to_string());
            }
            Statement::TSGlobalDeclaration(block) => {
                for (item, _) in items_in(&block.body.body) {
                    if let TopLevelItem::Interface(d) = item {
                        names.push(d.id.name.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    names
}

// What a `namespace N { ... }` holds, as the same flat list any statement list gives.
//
// Problem: `namespace A.B { ... }` has no block of its own. Its body is another namespace
// declaration, so a function that wanted a block found none, and the whole declaration was
// reported as not checked.
// Picked: read it as what it means, `namespace A { export namespace B { ... } }`: the
// members of A are one exported namespace, B, and every pass that walks a namespace walks
// that nesting by recursing into B like into any other member. `A.B.x` and `A.B.T` then
// need nothing the nested form does not already have.
// Cost: the exported mark on B is made up here, since the dotted form has none to read.
pub(super) fn namespace_members<'a>(
    ns: &'a TSNamespaceDeclaration<'a>,
) -> Vec<(TopLevelItem<'a>, bool)> {
    match &ns.body {
        TSNamespaceDeclarationBody::TSModuleBlock(block) => items_in(&block.body),
        TSNamespaceDeclarationBody::TSNamespaceDeclaration(inner) => {
            vec![(TopLevelItem::Namespace(inner), true)]
        }
    }
}

// Every declaration in a statement list, each with whether it is exported. A namespace's
// members are read through the same list, so it means the same inside and outside one.
pub(super) fn items_in<'a>(statements: &'a [Statement<'a>]) -> Vec<(TopLevelItem<'a>, bool)> {
    let mut items = Vec::new();
    for stmt in statements {
        match stmt {
            Statement::TSTypeAliasDeclaration(d) => items.push((TopLevelItem::Alias(d), false)),
            Statement::TSInterfaceDeclaration(d) => items.push((TopLevelItem::Interface(d), false)),
            Statement::ClassDeclaration(d) => items.push((TopLevelItem::Class(d), false)),
            Statement::TSEnumDeclaration(d) => items.push((TopLevelItem::Enum(d), false)),
            Statement::VariableDeclaration(d) => items.push((TopLevelItem::Variable(d), false)),
            Statement::FunctionDeclaration(d) => items.push((TopLevelItem::Function(d), false)),
            Statement::TSNamespaceDeclaration(d) => items.push((TopLevelItem::Namespace(d), false)),
            Statement::TSExternalModuleDeclaration(d) => {
                items.push((TopLevelItem::ExternalModule(d), false))
            }
            // `declare global { ... }`: its members are declared at the top level of the file,
            // where the rest of the file finds them. They are also the project's globals,
            // which the export collector reads separately.
            Statement::TSGlobalDeclaration(d) => items.extend(items_in(&d.body.body)),
            Statement::ExportDeclaration(export) => match &export.declaration {
                Declaration::TSTypeAliasDeclaration(d) => {
                    items.push((TopLevelItem::Alias(d), true))
                }
                Declaration::TSInterfaceDeclaration(d) => {
                    items.push((TopLevelItem::Interface(d), true))
                }
                Declaration::ClassDeclaration(d) => items.push((TopLevelItem::Class(d), true)),
                Declaration::TSEnumDeclaration(d) => items.push((TopLevelItem::Enum(d), true)),
                Declaration::VariableDeclaration(d) => {
                    items.push((TopLevelItem::Variable(d), true))
                }
                Declaration::FunctionDeclaration(d) => {
                    items.push((TopLevelItem::Function(d), true))
                }
                Declaration::TSNamespaceDeclaration(d) => {
                    items.push((TopLevelItem::Namespace(d), true))
                }
                _ => {}
            },
            Statement::ExportDefaultDeclaration(export) => match &export.declaration {
                ExportDefaultDeclarationKind::FunctionDeclaration(d) => {
                    items.push((TopLevelItem::Function(d), true))
                }
                ExportDefaultDeclarationKind::ClassDeclaration(d) => {
                    items.push((TopLevelItem::Class(d), true))
                }
                ExportDefaultDeclarationKind::TSInterfaceDeclaration(d) => {
                    items.push((TopLevelItem::Interface(d), true))
                }
                _ => {}
            },
            _ => {}
        }
    }
    items
}

// Two passes over the same statement list. The first registers every named type
// (interfaces, aliases, classes, enums) before anything is resolved, so a
// function declared earlier in the file can reference a type declared later. The
// second pass resolves function and variable signatures, which can look up
// any of those names regardless of source order.
pub fn declare_top_level<'ast>(program: &'ast Program<'ast>, ctx: &mut CheckContext<'ast, '_>) {
    let items = items_in(&program.body);
    declare_types(&items, ctx);
    super::imports::merge_own_global_interfaces(program, ctx);
    super::ambient_checks::check(program, ctx);
    for (name, span, involves_enum) in ctx.namespace.take_declaration_collisions() {
        let message = if involves_enum {
            crate::diagnostic_messages::messages::enum_declaration_merge()
        } else {
            crate::diagnostic_messages::messages::duplicate_type_declaration(&name)
        };
        ctx.error(message, span);
    }
    declare_values(&items, ctx, program.source_type.is_typescript_definition());
    resolve_merged_declarations(&items, ctx);
    // tsc reads every alias, used or not, so an error inside one (`[A, B][2]`) is reported
    // for an alias nothing refers to.
    for (item, _) in items.iter().copied() {
        if let TopLevelItem::Alias(alias) = item
            && alias.type_parameters.is_none()
        {
            let _ = ctx
                .namespace
                .resolve(alias.id.name.as_str(), &mut ctx.arena);
        }
    }
}

// tsc checks a merged interface (several `interface A` parts, or a class with an interface of
// its name) whether or not anything refers to it, so a conflicting property (TS2717, TS2687)
// is reported for an unused one as well. The conflicts are found when the parts are combined,
// which otherwise happens only when something first resolves the name.
fn resolve_merged_declarations<'ast>(
    items: &[(TopLevelItem<'ast>, bool)],
    ctx: &mut CheckContext<'ast, '_>,
) {
    let mut parts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for (item, _) in items.iter().copied() {
        match item {
            TopLevelItem::Interface(decl) => {
                *parts.entry(decl.id.name.to_string()).or_default() += 1
            }
            TopLevelItem::Class(class) => {
                if let Some(id) = &class.id {
                    *parts.entry(id.name.to_string()).or_default() += 1;
                }
            }
            _ => {}
        }
    }
    for (name, count) in parts {
        if count > 1 {
            let _ = ctx.namespace.resolve(&name, &mut ctx.arena);
        }
    }
}

// First pass over a statement list: every named type, so the rest can refer to it.
// A namespace's members are registered the same way, under its name.
fn declare_types<'ast>(items: &[(TopLevelItem<'ast>, bool)], ctx: &mut CheckContext<'ast, '_>) {
    for (item, exported) in items.iter().copied() {
        ctx.namespace.set_declaration_exported(exported);
        match item {
            TopLevelItem::Alias(decl) => {
                ctx.namespace.insert_type_alias(
                    &decl.id.name,
                    &decl.type_annotation,
                    decl.type_parameters.as_deref(),
                    decl.id.span,
                );
                if let Some(symbol_id) = decl.id.symbol_id.get() {
                    ctx.symbol_capabilities
                        .entry(symbol_id)
                        .or_default()
                        .has_type = true;
                }
            }
            TopLevelItem::Interface(decl) => {
                ctx.namespace.insert_interface(&decl.id.name, decl);
                if let Some(symbol_id) = decl.id.symbol_id.get() {
                    ctx.symbol_capabilities
                        .entry(symbol_id)
                        .or_default()
                        .has_type = true;
                }
            }
            TopLevelItem::Class(class) => {
                if let Some(id) = &class.id {
                    ctx.namespace.insert_class(&id.name, class);
                    if let Some(symbol_id) = id.symbol_id.get() {
                        let spaces = ctx.symbol_capabilities.entry(symbol_id).or_default();
                        spaces.has_type = true;
                        spaces.has_value = true;
                        spaces.value_available = true;
                    }
                }
            }
            TopLevelItem::Enum(decl) => declare_enum(decl, ctx),
            TopLevelItem::Namespace(ns) => {
                ctx.namespace.push_scope(&ns.id.name);
                declare_types(&namespace_members(ns), ctx);
                ctx.namespace.pop_scope();
            }
            TopLevelItem::ExternalModule(module) => {
                if let Some(body) = &module.body {
                    ctx.namespace.push_scope(&module_scope(&module.id.value));
                    declare_types(&items_in(&body.body), ctx);
                    ctx.namespace.pop_scope();
                }
            }
            _ => {}
        }
    }
}

// Second pass: signatures, which can name any type regardless of source order. A
// namespace is declared as a value (an object of its exported members) after its own
// members are, and only when that object is certain to be whole.
fn declare_values<'ast>(
    items: &[(TopLevelItem<'ast>, bool)],
    ctx: &mut CheckContext<'ast, '_>,
    ambient: bool,
) {
    let overload_sets = overload_sets(items);
    for (item, _) in items.iter().copied() {
        match item {
            TopLevelItem::Variable(decl) => {
                for declarator in &decl.declarations {
                    let BindingPattern::BindingIdentifier(id) = &declarator.id else {
                        continue;
                    };
                    let Some(annotation) = &declarator.type_annotation else {
                        continue;
                    };
                    let Some(symbol_id) = id.symbol_id.get() else {
                        continue;
                    };
                    if let Some(type_id) =
                        resolve_type_annotation(annotation, &mut ctx.namespace, &mut ctx.arena)
                    {
                        ctx.declare_value(symbol_id, &id.name, type_id);
                    }
                }
            }
            TopLevelItem::Function(func) => {
                // A signature in an overload set is declared with the set, by the first of
                // them; any other function is declared on its own.
                if declare_overload_set(func, &overload_sets, ctx).is_none() {
                    declare_function(func, ctx);
                }
            }
            TopLevelItem::Class(class) => declare_class_constructor(class, items, ctx),
            TopLevelItem::ExternalModule(module) => {
                if let Some(body) = &module.body {
                    ctx.namespace.push_scope(&module_scope(&module.id.value));
                    declare_values(&items_in(&body.body), ctx, true);
                    ctx.namespace.pop_scope();
                }
            }
            // A namespace is declared below, once every other declaration of its name has
            // been, so that all of its blocks are read together.
            TopLevelItem::Namespace(_) => {}
            _ => {}
        }
    }
    declare_namespace_values(items, ctx, ambient);
}

// The value of each namespace in `items`, one per name however many blocks wrote it.
//
// Problem: a namespace was declared as an object only when nothing else shared its name.
// `function f() {}` with `namespace f { export const extra = 1 }` (the way older code
// hangs properties on a function) left `f` as the bare function, so `f.extra` was
// reported as missing; a second block of the same namespace left the name unknown.
// Picked: the blocks of one name are one object, and when a function, a class or an
// enum has that name too, the value is the intersection of its own type and that object:
// `f()` still calls, `new C()` still constructs, and `f.extra` reads the member. The
// intersection is built after every other declaration of the name, so the order they are
// written in does not matter.
// Cost: a name shared with a variable is invalid TypeScript and is left as it was. If
// any member of any block has no type yet, nothing is declared and the name keeps what the
// other declaration gave it, since an object missing a member would report one that exists.
fn declare_namespace_values<'ast>(
    items: &[(TopLevelItem<'ast>, bool)],
    ctx: &mut CheckContext<'ast, '_>,
    ambient: bool,
) {
    let mut done: Vec<&str> = Vec::new();
    for (item, _) in items.iter().copied() {
        let TopLevelItem::Namespace(first) = item else {
            continue;
        };
        let name = first.id.name.as_str();
        if done.contains(&name) {
            continue;
        }
        done.push(name);

        // Every block of this name, and everything written in them as one list, so a
        // namespace nested in two blocks (`A.B` in one, `namespace B` inside `A` in the
        // other) is met as two blocks of B and merged the same way.
        let blocks: Vec<&TSNamespaceDeclaration<'ast>> = items
            .iter()
            .filter_map(|(other, _)| match other {
                TopLevelItem::Namespace(block) if block.id.name == name => Some(*block),
                _ => None,
            })
            .collect();
        let all_members: Vec<(TopLevelItem<'ast>, bool)> =
            blocks.iter().flat_map(|b| namespace_members(b)).collect();
        ctx.namespace.push_scope(name);
        declare_values(
            &all_members,
            ctx,
            ambient || blocks.iter().any(|b| b.declare),
        );
        ctx.namespace.pop_scope();

        let mut properties: Vec<crate::types::PropertyEntry> = Vec::new();
        let mut targets: Vec<oxc_semantic::SymbolId> = Vec::new();
        let enum_names: std::collections::HashSet<String> = all_members
            .iter()
            .filter_map(|(item, _)| match item {
                TopLevelItem::Enum(decl) => Some(decl.id.name.to_string()),
                _ => None,
            })
            .collect();
        let mut whole = true;
        for block in blocks.iter().copied() {
            match namespace_properties(&namespace_members(block), ambient || block.declare, ctx) {
                Some(found) => {
                    for property in found {
                        if let Some(existing) = properties
                            .iter_mut()
                            .find(|existing| existing.name == property.name)
                        {
                            if enum_names.contains(property.name.as_ref()) {
                                match merge_enum_value_shapes(
                                    &mut ctx.arena,
                                    property.name.as_ref(),
                                    existing.type_id,
                                    property.type_id,
                                ) {
                                    Some(merged) => existing.type_id = merged,
                                    None => whole = false,
                                }
                            }
                        } else {
                            properties.push(property);
                        }
                    }
                }
                None => whole = false,
            }
            if let Some(symbol) = block.id.symbol_id.get() {
                if !targets.contains(&symbol) {
                    targets.push(symbol);
                }
            }
        }
        if !whole {
            continue;
        }

        // The declarations the namespace merges with, other than its own blocks.
        let mut base: Option<crate::arena::TypeId> = None;
        let mut mergeable = true;
        for (other, _) in items.iter().copied() {
            let symbol = match other {
                TopLevelItem::Function(f) => f.id.as_ref().filter(|id| id.name == name),
                TopLevelItem::Class(c) => c.id.as_ref().filter(|id| id.name == name),
                TopLevelItem::Enum(e) => Some(&e.id).filter(|id| id.name == name),
                TopLevelItem::Variable(_) if declares_value_named(&other, name) => {
                    mergeable = false;
                    None
                }
                _ => None,
            }
            .and_then(|id| id.symbol_id.get());
            if let Some(symbol) = symbol {
                if base.is_some() {
                    mergeable = false;
                }
                match ctx.symbols.get(symbol) {
                    Some(type_id) => base = Some(type_id),
                    // The other declaration has no type (an unannotated signature), so
                    // there is nothing to put the object next to.
                    None => mergeable = false,
                }
                if !targets.contains(&symbol) {
                    targets.push(symbol);
                }
            }
        }
        if !mergeable {
            continue;
        }

        properties.sort_by(|a, b| a.name.cmp(&b.name));
        properties.dedup_by(|a, b| a.name == b.name);
        let object = ctx
            .arena
            .alloc(Type::Object(crate::types::ObjectType::new(properties)));
        let value = match base {
            None => object,
            Some(base) => match ctx.arena.alloc_intersection(vec![base, object]) {
                Ok(merged) => merged,
                Err(_) => continue,
            },
        };
        for symbol in targets {
            ctx.symbols.declare(symbol, value);
        }
    }
}

// Whether `item` introduces a value called `name`, which a namespace of that name would
// merge with.
fn declares_value_named(item: &TopLevelItem<'_>, name: &str) -> bool {
    match item {
        TopLevelItem::Namespace(ns) => ns.id.name == name,
        TopLevelItem::Function(f) => f.id.as_ref().is_some_and(|id| id.name == name),
        TopLevelItem::Class(c) => c.id.as_ref().is_some_and(|id| id.name == name),
        TopLevelItem::Enum(e) => e.id.name == name,
        TopLevelItem::Variable(v) => v
            .declarations
            .iter()
            .any(|d| matches!(&d.id, BindingPattern::BindingIdentifier(id) if id.name == name)),
        TopLevelItem::Alias(_) | TopLevelItem::Interface(_) | TopLevelItem::ExternalModule(_) => {
            false
        }
    }
}

// One property per exported value member of a namespace block. None when any of them has no
// type yet (an unannotated variable is typed from its initializer later), because an object
// missing a member would report a property that exists as missing. In an ambient namespace
// every member is exported unless some is marked.
fn namespace_properties(
    members: &[(TopLevelItem<'_>, bool)],
    ambient: bool,
    ctx: &mut CheckContext<'_, '_>,
) -> Option<Vec<crate::types::PropertyEntry>> {
    let any_marked = members.iter().any(|(_, exported)| *exported);
    let mut properties: Vec<crate::types::PropertyEntry> = Vec::new();
    for (item, exported) in members.iter().copied() {
        if !(exported || (ambient && !any_marked)) {
            continue;
        }
        let mut add = |name: &str, symbol: Option<oxc_semantic::SymbolId>| -> Option<()> {
            let type_id = ctx.symbols.get(symbol?)?;
            properties.push(crate::types::PropertyEntry {
                name: name.into(),
                type_id,
                optional: false,
                is_method: false,
                modifiers: Default::default(),
            });
            Some(())
        };
        match item {
            TopLevelItem::Function(f) => {
                let id = f.id.as_ref()?;
                add(&id.name, id.symbol_id.get())?;
            }
            TopLevelItem::Class(c) => {
                let id = c.id.as_ref()?;
                add(&id.name, id.symbol_id.get())?;
            }
            TopLevelItem::Enum(e) => add(&e.id.name, e.id.symbol_id.get())?,
            TopLevelItem::Namespace(n) => add(&n.id.name, n.id.symbol_id.get())?,
            TopLevelItem::Variable(v) => {
                for d in &v.declarations {
                    let BindingPattern::BindingIdentifier(id) = &d.id else {
                        return None;
                    };
                    // No annotation means no type yet. A plain literal initializer is
                    // enough to know the member's type without checking the body, which
                    // lets `namespace C { export const tag = 'c' }` merge with `class C`.
                    let declared = id
                        .symbol_id
                        .get()
                        .and_then(|symbol| ctx.symbols.get(symbol));
                    let type_id = match declared {
                        Some(type_id) => type_id,
                        None => d.init.as_ref().and_then(|init| {
                            literal_initializer_type(init, v.kind.is_const(), &mut ctx.arena)
                        })?,
                    };
                    properties.push(crate::types::PropertyEntry {
                        name: id.name.as_str().into(),
                        type_id,
                        optional: false,
                        is_method: false,
                        modifiers: Default::default(),
                    });
                }
            }
            TopLevelItem::Alias(_)
            | TopLevelItem::Interface(_)
            | TopLevelItem::ExternalModule(_) => {}
        }
    }
    Some(properties)
}

// The type of a literal initializer: the literal itself for a `const`, its primitive for
// `let` and `var`. Anything that is not a plain literal returns `None`, so the caller keeps
// treating the member as one whose type is not known yet.
fn literal_initializer_type(
    init: &oxc_ast::ast::Expression<'_>,
    is_const: bool,
    arena: &mut crate::arena::TypeArena,
) -> Option<crate::arena::TypeId> {
    use oxc_ast::ast::Expression;
    match init.without_parentheses() {
        Expression::StringLiteral(text) => Some(if is_const {
            arena.alloc(Type::StringLiteral(text.value.to_string()))
        } else {
            arena.string()
        }),
        Expression::TemplateLiteral(template) if template.expressions.is_empty() => {
            let text = template.quasis.first()?.value.cooked.as_ref()?.to_string();
            Some(if is_const {
                arena.alloc(Type::StringLiteral(text))
            } else {
                arena.string()
            })
        }
        Expression::NumericLiteral(number) => Some(if is_const {
            arena.alloc(Type::NumberLiteral(number.value))
        } else {
            arena.number()
        }),
        Expression::BooleanLiteral(boolean) => Some(if is_const {
            arena.alloc(Type::BooleanLiteral(boolean.value))
        } else {
            arena.boolean()
        }),
        _ => None,
    }
}

// Signatures of one name written more than once, none of them with a body: `declare
// function f(x: string): string; declare function f(x: number): number;`.
//
// Problem: each declaration replaced the one before, so only the last signature was ever
// known and a call the first one accepts was checked against the last.
// Picked: the signatures are one value, an intersection in the order they were written,
// which is how a call already picks among several signatures (see check_callable).
// Cost: a name with an implementation next to its overload signatures is left as it was,
// the implementation's own type, since that signature is not part of what a caller sees
// and the body pass reads it. Only signature-only sets are merged.
struct OverloadSet<'a, 'ast> {
    symbol_id: oxc_semantic::SymbolId,
    signatures: Vec<&'a Function<'ast>>,
}

fn overload_sets<'a, 'ast>(items: &[(TopLevelItem<'ast>, bool)]) -> Vec<OverloadSet<'a, 'ast>>
where
    'ast: 'a,
{
    let mut sets: Vec<OverloadSet<'a, 'ast>> = Vec::new();
    let mut implemented: Vec<oxc_semantic::SymbolId> = Vec::new();
    for (item, _) in items.iter().copied() {
        let TopLevelItem::Function(func) = item else {
            continue;
        };
        let Some(symbol_id) = func.id.as_ref().and_then(|id| id.symbol_id.get()) else {
            continue;
        };
        if func.body.is_some() {
            implemented.push(symbol_id);
            continue;
        }
        match sets.iter_mut().find(|set| set.symbol_id == symbol_id) {
            Some(set) => set.signatures.push(func),
            None => sets.push(OverloadSet {
                symbol_id,
                signatures: vec![func],
            }),
        }
    }
    sets.retain(|set| set.signatures.len() > 1 && !implemented.contains(&set.symbol_id));
    sets
}

// Declares the whole overload set when `func` is one of its signatures. Returns whether it
// belongs to one; only the first signature of a set does the declaring.
fn declare_overload_set(
    func: &Function<'_>,
    sets: &[OverloadSet<'_, '_>],
    ctx: &mut CheckContext<'_, '_>,
) -> Option<()> {
    let symbol_id = func.id.as_ref().and_then(|id| id.symbol_id.get())?;
    let set = sets.iter().find(|set| set.symbol_id == symbol_id)?;
    if !std::ptr::eq(set.signatures[0], func) {
        return Some(());
    }
    let mut members = Vec::with_capacity(set.signatures.len());
    for signature in &set.signatures {
        // A signature that cannot be resolved leaves the name as the last one declared it,
        // as before: a set with a hole in it would reject calls that are fine.
        match resolve_function_type(signature, ctx) {
            Some(member) => members.push(member),
            None => {
                for signature in &set.signatures {
                    declare_function(signature, ctx);
                }
                return Some(());
            }
        }
    }
    match ctx.arena.alloc_intersection(members) {
        Ok(set_type) => ctx.symbols.declare(symbol_id, set_type),
        Err(_) => {
            for signature in &set.signatures {
                declare_function(signature, ctx);
            }
        }
    }
    Some(())
}

mod enums;
mod functions;
pub(super) use self::enums::*;
pub(super) use self::functions::*;
