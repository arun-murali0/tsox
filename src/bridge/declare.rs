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
    for (name, span, involves_enum) in ctx.namespace.take_declaration_collisions() {
        let message = if involves_enum {
            crate::diagnostic_messages::messages::enum_declaration_merge()
        } else {
            crate::diagnostic_messages::messages::duplicate_type_declaration(&name)
        };
        ctx.error(message, span);
    }
    declare_values(&items, ctx, program.source_type.is_typescript_definition());
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
            }
            TopLevelItem::Interface(decl) => {
                ctx.namespace.insert_interface(&decl.id.name, decl);
            }
            TopLevelItem::Class(class) => {
                if let Some(id) = &class.id {
                    ctx.namespace.insert_class(&id.name, class);
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
                        ctx.symbols.declare(symbol_id, type_id);
                    }
                }
            }
            TopLevelItem::Function(func) => declare_function(func, ctx),
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
        let mut whole = true;
        for block in blocks.iter().copied() {
            match namespace_properties(&namespace_members(block), ambient || block.declare, ctx) {
                Some(found) => properties.extend(found),
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
                    add(&id.name, id.symbol_id.get())?;
                }
            }
            TopLevelItem::Alias(_)
            | TopLevelItem::Interface(_)
            | TopLevelItem::ExternalModule(_) => {}
        }
    }
    Some(properties)
}

// Registers a function's signature under its symbol. Shared by the top-level pass
// and by check_function_declaration / hoist_function_declarations, which call it
// for a function declared inside another body: the same signature rules apply
// there, and nothing but this pass would ever give such a function a type.
//
// Problem: a nested `function helper(...) {}` was never declared, so its body was
// skipped without a word and every call to it read as an unknown type.
// Picked: declare it on the way into the enclosing body, before any statement in
// that body is checked, so a call written above the declaration still sees it
// (function declarations hoist).
pub(super) fn declare_function(func: &Function<'_>, ctx: &mut CheckContext<'_, '_>) {
    let Some(name) = func.id.as_ref() else {
        return;
    };
    let Some(symbol_id) = name.symbol_id.get() else {
        return;
    };
    // Problem: an `async function` with no return annotation had no signature, so it was never
    // declared and its body was never checked: every error inside it went unreported, and a call
    // to it was unknown.
    // Picked: a named one is declared with the same `any` placeholder as a sync function; the
    // body pass replaces it with a Promise of what the body returns.
    // Cost: until that replacement, a call to it is unchecked, as for a sync function.
    if let Some(function_type) = resolve_function_signature(func, ctx, true) {
        ctx.symbols.declare(symbol_id, function_type);
    }
}

/// Resolve a function's declared signature without requiring it to have a local symbol.
/// Export serialization uses this for overload signatures and named-default edge cases;
/// ordinary declarations still go through `declare_function` above.
pub(super) fn resolve_function_type(
    func: &Function<'_>,
    ctx: &mut CheckContext<'_, '_>,
) -> Option<crate::arena::TypeId> {
    resolve_function_signature(func, ctx, false)
}

// `async_placeholder` gives an unannotated `async function` the `any` placeholder a sync one
// gets. Only a named declaration with a body pass to replace it asks for that.
fn resolve_function_signature(
    func: &Function<'_>,
    ctx: &mut CheckContext<'_, '_>,
    async_placeholder: bool,
) -> Option<crate::arena::TypeId> {
    let scope = ctx.namespace.push_type_params(&mut ctx.arena, func);
    let params = resolve_function_params(&func.params, &mut ctx.namespace, &mut ctx.arena);
    let return_type = match &func.return_type {
        Some(rt) => resolve_type_annotation(rt, &mut ctx.namespace, &mut ctx.arena),
        // Without a return annotation this is only a placeholder. The body pass replaces
        // it for named functions, but export-only anonymous/overload signatures have no
        // symbol-backed body inference and must not claim that placeholder is a real API.
        None if func.generator => None,
        None if func.r#async && !async_placeholder => None,
        None => Some(ctx.arena.any()),
    };
    let type_params = resolve_function_type_params(
        func.type_parameters.as_deref(),
        &mut ctx.namespace,
        &mut ctx.arena,
    );
    ctx.namespace.pop_type_params(scope);
    let (Some(params), Some(return_type)) = (params, return_type) else {
        return None;
    };
    Some(
        ctx.arena
            .alloc(crate::types::Type::Function(crate::types::FunctionType {
                params,
                return_type,
                type_params,
                is_untyped: false,
            })),
    )
}

fn declare_class_constructor<'a>(
    class: &'a Class<'a>,
    items: &[(TopLevelItem<'a>, bool)],
    ctx: &mut CheckContext<'_, '_>,
) {
    let Some(name) = class.id.as_ref() else {
        return;
    };
    let Some(symbol_id) = name.symbol_id.get() else {
        return;
    };
    let instance_type = match ctx.namespace.resolve(&name.name, &mut ctx.arena) {
        crate::namespace::Resolution::Resolved(type_id) => type_id,
        _ => return,
    };
    let class_scope = ctx
        .namespace
        .push_decl_type_params(&mut ctx.arena, class.type_parameters.as_deref());
    let constructor_type_params = resolve_function_type_params(
        class.type_parameters.as_deref(),
        &mut ctx.namespace,
        &mut ctx.arena,
    );
    let (constructor_params, constructor_is_untyped) = constructor_params_of(class, items, ctx);
    ctx.namespace.pop_type_params(class_scope);
    let constructor_type =
        ctx.arena
            .alloc(crate::types::Type::Function(crate::types::FunctionType {
                params: constructor_params,
                return_type: instance_type,
                type_params: constructor_type_params,
                is_untyped: constructor_is_untyped,
            }));
    ctx.symbols.declare(symbol_id, constructor_type);
}

// The parameters `new C(...)` takes. A class with a constructor takes that one's. A class
// without one takes what the class it extends takes, so `class D extends B { }` is built
// with whatever B's constructor wants.
//
// Problem: such a class was given no parameters, so `new D(1)` was TS2554 when B takes one.
// Picked: walk up `extends` through the classes of this file to the first constructor. What
// cannot be followed answers "untyped", which accepts any arguments: a base that is not in
// this list (imported, global, an expression), a generic base (its parameters may name its
// own type parameters, which mean something else here), or a chain too long to be real. A
// wrong guess can then only hide an error, never invent one.
fn constructor_params_of<'a>(
    class: &'a Class<'a>,
    items: &[(TopLevelItem<'a>, bool)],
    ctx: &mut CheckContext<'_, '_>,
) -> (Vec<crate::types::Param>, bool) {
    const UNTYPED: (Vec<crate::types::Param>, bool) = (Vec::new(), true);
    let mut current = class;
    for _ in 0..32 {
        if let Some(ctor) = find_constructor(current) {
            return match resolve_function_params(&ctor.params, &mut ctx.namespace, &mut ctx.arena) {
                Some(params) => (params, false),
                None => UNTYPED,
            };
        }
        let Some(heritage) = &current.heritage else {
            return (Vec::new(), false);
        };
        let oxc_ast::ast::Expression::Identifier(base_name) = &heritage.expression else {
            return UNTYPED;
        };
        let base = items.iter().find_map(|(item, _)| match item {
            TopLevelItem::Class(candidate)
                if candidate
                    .id
                    .as_ref()
                    .is_some_and(|id| id.name == base_name.name) =>
            {
                Some(*candidate)
            }
            _ => None,
        });
        match base {
            Some(base) if base.type_parameters.is_none() => current = base,
            _ => return UNTYPED,
        }
    }
    UNTYPED
}

pub(super) fn find_constructor<'a>(
    class: &'a oxc_ast::ast::Class<'a>,
) -> Option<&'a oxc_ast::ast::Function<'a>> {
    use oxc_ast::ast::{ClassElement, MethodDefinitionKind};
    class.body.body.iter().find_map(|element| {
        let ClassElement::MethodDefinition(method) = element else {
            return None;
        };
        (method.kind == MethodDefinitionKind::Constructor).then_some(&*method.value)
    })
}

fn declare_enum(decl: &oxc_ast::ast::TSEnumDeclaration, ctx: &mut CheckContext<'_, '_>) {
    let Some(symbol_id) = decl.id.symbol_id.get() else {
        return;
    };
    let Some(members) = resolve_enum_members(decl, &mut ctx.arena) else {
        return;
    };

    // A TypeScript enum is really two separate names sharing one identifier: used
    // in a type position, it means the union of its member literal types; used in
    // a value position, it means an object whose properties are its members. Both
    // are built from the same resolved member list.
    let (type_position, value_type) = ctx.arena.build_enum(&decl.id.name, members);
    ctx.namespace
        .insert_enum(&decl.id.name, type_position, decl.id.span);
    ctx.symbols.declare(symbol_id, value_type);
}

fn resolve_enum_members(
    decl: &oxc_ast::ast::TSEnumDeclaration,
    arena: &mut crate::arena::TypeArena,
) -> Option<Vec<(String, crate::arena::TypeId)>> {
    use oxc_ast::ast::TSEnumMemberName;

    let mut members = Vec::with_capacity(decl.body.members.len());
    // Every numeric member's value so far, by name, so a later initializer can
    // refer to an earlier member (`ReadWrite = Read | Write`).
    let mut numeric_values: Vec<(String, f64)> = Vec::new();
    // Tracks the running value for TypeScript's own auto-increment rule: a
    // numeric member with no initializer takes the previous numeric member's
    // value plus one. Reset to None whenever a string member breaks that chain,
    // since auto-increment only continues across consecutive numeric members.
    let mut prev_numeric: Option<f64> = None;

    for member in &decl.body.members {
        let name = match &member.id {
            TSEnumMemberName::Identifier(id) => id.name.to_string(),
            TSEnumMemberName::String(s) => s.value.to_string(),

            TSEnumMemberName::ComputedString(_) | TSEnumMemberName::ComputedTemplateString(_) => {
                // A computed member name means the enum's shape cannot be known
                // without evaluating an expression, so the whole enum is left
                // unresolved rather than guessing.
                return None;
            }
        };

        let member_type = match &member.initializer {
            Some(Expression::StringLiteral(s)) => {
                prev_numeric = None;
                arena.alloc(Type::StringLiteral(s.value.to_string()))
            }

            // Any other initializer is only usable if it is a constant
            // expression this checker can fully evaluate, as tsc does for
            // `1 << 0` or `Read | Write`. One it cannot evaluate, such as a
            // function call, leaves the whole enum unresolved rather than
            // guessing a value.
            Some(initializer) => {
                let value = eval_enum_constant(initializer, &numeric_values)?;
                prev_numeric = Some(value);
                numeric_values.push((name.clone(), value));
                arena.alloc(Type::NumberLiteral(value))
            }
            None => {
                let next = match prev_numeric {
                    Some(n) => n + 1.0,
                    None if members.is_empty() => 0.0,

                    None => return None,
                };
                prev_numeric = Some(next);
                numeric_values.push((name.clone(), next));
                arena.alloc(Type::NumberLiteral(next))
            }
        };

        members.push((name, member_type));
    }

    Some(members)
}

// Evaluates the constant numeric expressions tsc accepts as an enum member
// initializer: numeric literals, parentheses, unary + - ~, the arithmetic and
// bitwise binary operators, and references to earlier members of the same enum.
// Anything else, or a result that is not a finite number, is None, which leaves
// the enum unresolved the same way an unsupported initializer always has.
fn eval_enum_constant(expr: &Expression, known: &[(String, f64)]) -> Option<f64> {
    let value = match expr {
        Expression::NumericLiteral(n) => n.value,
        Expression::ParenthesizedExpression(inner) => eval_enum_constant(&inner.expression, known)?,
        Expression::Identifier(id) => known.iter().find(|(name, _)| name == id.name.as_str())?.1,
        Expression::UnaryExpression(unary) => {
            let operand = eval_enum_constant(&unary.argument, known)?;
            match unary.operator {
                UnaryOperator::UnaryPlus => operand,
                UnaryOperator::UnaryNegation => -operand,
                UnaryOperator::BitwiseNot => f64::from(!to_int32(operand)),
                _ => return None,
            }
        }
        Expression::BinaryExpression(binary) => {
            let left = eval_enum_constant(&binary.left, known)?;
            let right = eval_enum_constant(&binary.right, known)?;
            match binary.operator {
                BinaryOperator::Addition => left + right,
                BinaryOperator::Subtraction => left - right,
                BinaryOperator::Multiplication => left * right,
                BinaryOperator::Division => left / right,
                BinaryOperator::Remainder => left % right,
                BinaryOperator::Exponential => left.powf(right),
                BinaryOperator::ShiftLeft => {
                    f64::from(to_int32(left).wrapping_shl(to_uint32(right) & 31))
                }
                BinaryOperator::ShiftRight => {
                    f64::from(to_int32(left).wrapping_shr(to_uint32(right) & 31))
                }
                BinaryOperator::ShiftRightZeroFill => {
                    f64::from(to_uint32(left).wrapping_shr(to_uint32(right) & 31))
                }
                BinaryOperator::BitwiseAnd => f64::from(to_int32(left) & to_int32(right)),
                BinaryOperator::BitwiseOR => f64::from(to_int32(left) | to_int32(right)),
                BinaryOperator::BitwiseXOR => f64::from(to_int32(left) ^ to_int32(right)),
                _ => return None,
            }
        }
        _ => return None,
    };
    value.is_finite().then_some(value)
}

// JavaScript's ToUint32 and ToInt32: truncate, then wrap modulo 2^32. The bitwise
// operators and shifts are defined on these 32-bit views of a number, not on the
// f64 itself.
fn to_uint32(n: f64) -> u32 {
    if !n.is_finite() {
        return 0;
    }
    n.trunc().rem_euclid(4_294_967_296.0) as u32
}

fn to_int32(n: f64) -> i32 {
    to_uint32(n) as i32
}
