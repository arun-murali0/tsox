//! Builders turning one export item into a pending entry (function, class, variable, enum, namespace).
use super::*;

pub(super) fn note_dropped_side(
    name: &str,
    kind: DeclKind,
    types: &mut Vec<String>,
    values: &mut Vec<String>,
) {
    let into = if matches!(kind, DeclKind::Interface | DeclKind::Alias) {
        types
    } else {
        values
    };
    if !into.iter().any(|existing| existing == name) {
        into.push(name.to_string());
    }
}

// A generic interface, class or alias that could not be written out still has a parameter
// list. Keep it, so that a bare `Array` or a `Map<string>` can be reported as the wrong number
// of type arguments even though nothing is known about the type itself.
pub(super) fn note_dropped_generic(
    ctx: &CheckContext<'_, '_>,
    name: &str,
    kind: DeclKind,
    into: &mut Vec<(String, Vec<(String, bool)>)>,
) {
    if !matches!(
        kind,
        DeclKind::Interface | DeclKind::Class | DeclKind::Alias
    ) || into.iter().any(|(existing, _)| existing == name)
    {
        return;
    }
    if let Some(decl) = ctx.namespace.declared_type_param_decl(name) {
        into.push((
            name.to_string(),
            decl.params
                .iter()
                .map(|param| (param.name.name.to_string(), param.default.is_some()))
                .collect(),
        ));
    }
}

pub(super) fn interface_or_alias(
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

pub(super) fn function<'ast>(
    ctx: &mut CheckContext<'ast, '_>,
    module: ModuleKey,
    f: &'ast Function<'ast>,
    item: &Item<'_>,
) -> Option<Pending> {
    if item.overloaded {
        let body = super::super::declare::resolve_function_type(f, ctx)?;
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

pub(super) fn default_function<'ast>(
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
        .or_else(|| super::super::declare::resolve_function_type(function, ctx))?;
    with_type_params(
        ctx,
        pending_of(module, item, body),
        function.type_parameters.as_deref(),
    )
}

pub(super) fn default_class_instance<'ast>(
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

pub(super) fn default_class_value<'ast>(
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
    let (params, is_untyped) = match super::super::declare::find_constructor(class) {
        Some(constructor) => match super::super::super::type_annotation::resolve_function_params(
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

pub(super) fn variable(
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
pub(super) fn enumeration(
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
pub(super) fn push_namespace<'ast>(
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

pub(super) fn push_namespace_types<'ast>(
    items: &mut Vec<Item<'ast>>,
    seen: &mut FxHashMap<(String, u8), u32>,
    ns: &'ast oxc_ast::ast::TSNamespaceDeclaration<'ast>,
    prefix: &str,
    outer_names: &[String],
    ambient: bool,
) {
    use super::super::declare::TopLevelItem;
    let members = super::super::declare::namespace_members(ns);
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
pub(super) fn push_class<'ast>(
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

pub(super) fn pending_of(module: ModuleKey, item: &Item<'_>, body: TypeId) -> Pending {
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
