//! Function and class-constructor declaration: signatures, overloads, predicates.
use super::*;

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
pub(in crate::bridge) fn declare_function(func: &Function<'_>, ctx: &mut CheckContext<'_, '_>) {
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
    let Some(function_type) = resolve_function_signature(func, ctx, true) else {
        return;
    };
    // Problem: `function f(a: string): number; function f(a: any): any {}` was typed by the last
    // declaration, the implementation, so a call saw its signature and none of the overloads: a
    // wrong argument count was reported where tsc picks the overload that fits, and a result was
    // `any` where tsc gives the overload's type.
    // Picked: every declaration without a body is an overload signature. A callee with more than
    // one is the intersection of them in the order written, which a call already resolves by
    // taking the first member that accepts it. The implementation is not callable by others, so it
    // is kept aside for the body check and does not replace the overloads.
    // Cost: an implementation signature that does not fit its overloads is not reported (TS2394).
    record_type_predicate(func, symbol_id, ctx);
    if func.body.is_none() {
        let overloads = ctx.overload_signatures.entry(symbol_id).or_default();
        overloads.push(function_type);
        let all = overloads.clone();
        let callee = if all.len() == 1 {
            function_type
        } else {
            ctx.arena.alloc_intersection(all).unwrap_or(function_type)
        };
        ctx.declare_value(symbol_id, &name.name, callee);
    } else if ctx.overload_signatures.contains_key(&symbol_id) {
        ctx.implementation_signatures
            .insert(symbol_id, function_type);
    } else {
        ctx.declare_value(symbol_id, &name.name, function_type);
    }
}

/// Resolve a function's declared signature without requiring it to have a local symbol.
/// Export serialization uses this for overload signatures and named-default edge cases;
/// ordinary declarations still go through `declare_function` above.
pub(in crate::bridge) fn resolve_function_type(
    func: &Function<'_>,
    ctx: &mut CheckContext<'_, '_>,
) -> Option<crate::arena::TypeId> {
    resolve_function_signature(func, ctx, false)
}

// `async_placeholder` gives an unannotated `async function` the `any` placeholder a sync one
// gets. Only a named declaration with a body pass to replace it asks for that.
// `function f(x): x is T` and `asserts x [is T]`: remember which parameter the predicate is
// about and the type it claims, for the narrowing a call to `f` does.
pub(in crate::bridge) fn record_type_predicate(
    func: &Function<'_>,
    symbol_id: oxc_semantic::SymbolId,
    ctx: &mut CheckContext<'_, '_>,
) {
    use oxc_ast::ast::{TSType, TSTypePredicateName};
    let Some(annotation) = &func.return_type else {
        return;
    };
    let TSType::TSTypePredicate(predicate) = &annotation.type_annotation else {
        return;
    };
    let TSTypePredicateName::Identifier(name) = &predicate.parameter_name else {
        return;
    };
    let Some(param_index) = func.params.items.iter().position(|param| {
        matches!(&param.pattern, oxc_ast::ast::BindingPattern::BindingIdentifier(id) if id.name == name.name)
    }) else {
        return;
    };
    let scope = ctx.namespace.push_type_params(&mut ctx.arena, func);
    let type_id = match &predicate.type_annotation {
        Some(annotation) => {
            let resolved = resolve_type_annotation(annotation, &mut ctx.namespace, &mut ctx.arena);
            if resolved.is_none() {
                ctx.namespace.pop_type_params(scope);
                return;
            }
            resolved
        }
        None => None,
    };
    ctx.namespace.pop_type_params(scope);
    ctx.type_predicates.insert(
        symbol_id,
        super::super::context::TypePredicate {
            param_index,
            type_id,
            asserts: predicate.asserts,
        },
    );
}

pub(in crate::bridge) fn resolve_overload_params_lenient(
    params: &oxc_ast::ast::FormalParameters<'_>,
    ctx: &mut CheckContext<'_, '_>,
) -> Option<Vec<crate::types::Param>> {
    let mut out = Vec::new();
    for param in &params.items {
        let type_id = match &param.type_annotation {
            Some(a) => resolve_type_annotation(a, &mut ctx.namespace, &mut ctx.arena)?,
            None => ctx.arena.any(),
        };
        out.push(crate::types::Param {
            type_id,
            optional: param.optional || param.initializer.is_some(),
            rest: false,
            name: None,
        });
    }
    if let Some(rest) = &params.rest {
        let type_id = match &rest.type_annotation {
            Some(a) => resolve_type_annotation(a, &mut ctx.namespace, &mut ctx.arena)?,
            None => ctx.arena.any(),
        };
        out.push(crate::types::Param {
            type_id,
            optional: false,
            rest: true,
            name: None,
        });
    }
    Some(out)
}

// A JavaScript file is not given the TypeScript rule above: its constructor functions get their
// properties from `this.x = ...` in the body, which this checker does not model, so checking a
// body there would report every one of them as missing.
// A body that reads `this` is left out as well: `function C(x) { this.p = x }` is a constructor
// function in tsc's eyes, and the checker has no type for `this` there, so it would report each
// property as missing. A nested non-arrow function has a `this` of its own and does not count.
pub(in crate::bridge) fn body_uses_this(func: &Function<'_>) -> bool {
    use oxc_ast_visit::Visit;
    struct Finder(bool);
    impl<'a> Visit<'a> for Finder {
        fn visit_this_expression(&mut self, _: &oxc_ast::ast::ThisExpression) {
            self.0 = true;
        }
        fn visit_function(&mut self, _: &Function<'a>, _: oxc_semantic::ScopeFlags) {}
    }
    let mut finder = Finder(false);
    if let Some(body) = &func.body {
        finder.visit_function_body(body);
    }
    finder.0
}

pub(in crate::bridge) fn is_javascript_file(file_name: &str) -> bool {
    [".js", ".jsx", ".mjs", ".cjs"]
        .iter()
        .any(|extension| file_name.ends_with(extension))
}

pub(in crate::bridge) fn resolve_function_signature(
    func: &Function<'_>,
    ctx: &mut CheckContext<'_, '_>,
    async_placeholder: bool,
) -> Option<crate::arena::TypeId> {
    let scope = ctx.namespace.push_type_params(&mut ctx.arena, func);
    let mut params = resolve_function_params(&func.params, &mut ctx.namespace, &mut ctx.arena);
    if params.is_none()
        && (func.body.is_none() || (!is_javascript_file(ctx.file_name) && !body_uses_this(func)))
    {
        // A parameter left unannotated is `any` in tsc (the implicit-any error is reported on
        // its own), for an overload signature and for a function with a body alike. Without
        // this a function with one untyped parameter had no signature, its body was never
        // checked, and nothing declared inside it had a type to read back.
        params = resolve_overload_params_lenient(&func.params, ctx);
    }
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

// The static members riding next to a class's constructor in its value type.
pub(in crate::bridge) fn static_members_of(
    arena: &crate::arena::TypeArena,
    value: crate::arena::TypeId,
) -> Option<Vec<crate::types::PropertyEntry>> {
    let crate::types::Type::Intersection(members) = arena.get(value) else {
        return None;
    };
    members.iter().find_map(|member| match arena.get(*member) {
        crate::types::Type::Object(object) => Some(object.properties.to_vec()),
        _ => None,
    })
}

pub(in crate::bridge) fn declare_class_constructor<'a>(
    class: &'a Class<'a>,
    items: &[(TopLevelItem<'a>, bool)],
    ctx: &mut CheckContext<'a, '_>,
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
    // The statics ride next to the constructor in one value type, so `Widget.create()` finds
    // them and `new Widget()` still finds the constructor.
    let class_scope = ctx.namespace.push_decl_type_params(&mut ctx.arena, None);
    let own_statics = ctx.namespace.resolve_class_statics(class, &mut ctx.arena);
    ctx.namespace.pop_type_params(class_scope);
    // A class also has the statics of the class it extends, when that one is declared above it
    // in this file; its own win by name.
    let inherited = match class.heritage.as_ref().map(|heritage| &heritage.expression) {
        Some(oxc_ast::ast::Expression::Identifier(base)) => {
            let query = crate::type_annotation::typeof_value_name(&base.name);
            match ctx.namespace.resolve(&query, &mut ctx.arena) {
                crate::namespace::Resolution::Resolved(base_value) => {
                    static_members_of(&ctx.arena, base_value)
                }
                _ => None,
            }
        }
        _ => None,
    };
    let statics = match (inherited, own_statics) {
        (Some(base), Some(own)) => {
            let mut properties = base;
            if let crate::types::Type::Object(own_object) = ctx.arena.get(own) {
                for property in own_object.properties.iter() {
                    match properties.iter_mut().find(|p| p.name == property.name) {
                        Some(existing) => *existing = property.clone(),
                        None => properties.push(property.clone()),
                    }
                }
            }
            Some(
                ctx.arena
                    .alloc(crate::types::Type::Object(crate::types::ObjectType::new(
                        properties,
                    ))),
            )
        }
        (Some(base), None) => Some(ctx.arena.alloc(crate::types::Type::Object(
            crate::types::ObjectType::new(base),
        ))),
        (None, own) => own,
    };
    // Constructor overloads: `new C(...)` is checked against each signature, and the
    // implementation signature (the one with a body) is not one of them. One signature, or
    // none, is the plain constructor above.
    let mut constructors = vec![constructor_type];
    {
        use oxc_ast::ast::{ClassElement, MethodDefinitionKind};
        let signatures: Vec<_> = class
            .body
            .body
            .iter()
            .filter_map(|element| match element {
                ClassElement::MethodDefinition(method)
                    if method.kind == MethodDefinitionKind::Constructor
                        && method.value.body.is_none() =>
                {
                    Some(&*method.value)
                }
                _ => None,
            })
            .collect();
        if signatures.len() > 1 {
            let scope = ctx
                .namespace
                .push_decl_type_params(&mut ctx.arena, class.type_parameters.as_deref());
            let mut overloads = Vec::new();
            for signature in &signatures {
                match resolve_function_params(&signature.params, &mut ctx.namespace, &mut ctx.arena)
                {
                    Some(params) => {
                        let type_params = constructor_type_params_for_overload(class, ctx);
                        overloads.push(ctx.arena.alloc(crate::types::Type::Function(
                            crate::types::FunctionType {
                                params,
                                return_type: instance_type,
                                type_params,
                                is_untyped: false,
                            },
                        )));
                    }
                    None => {
                        overloads.clear();
                        break;
                    }
                }
            }
            ctx.namespace.pop_type_params(scope);
            if !overloads.is_empty() {
                constructors = overloads;
            }
        }
    }
    // A class with no constructor of its own takes the overloads of the class it extends.
    if find_constructor(class).is_none()
        && class.type_parameters.is_none()
        && let Some(oxc_ast::ast::Expression::Identifier(base)) =
            class.heritage.as_ref().map(|heritage| &heritage.expression)
    {
        let query = crate::type_annotation::typeof_value_name(&base.name);
        if let crate::namespace::Resolution::Resolved(base_value) =
            ctx.namespace.resolve(&query, &mut ctx.arena)
            && let crate::types::Type::Intersection(parts) = ctx.arena.get(base_value).clone()
        {
            let functions: Vec<_> = parts
                .iter()
                .filter_map(|&part| match ctx.arena.get(part) {
                    crate::types::Type::Function(f) if f.type_params.is_empty() => Some(f.clone()),
                    _ => None,
                })
                .collect();
            if functions.len() > 1 {
                constructors = functions
                    .into_iter()
                    .map(|mut f| {
                        f.return_type = instance_type;
                        ctx.arena.alloc(crate::types::Type::Function(f))
                    })
                    .collect();
            }
        }
    }
    let mut members = constructors;
    members.extend(statics);
    let value_type = if members.len() == 1 {
        members[0]
    } else {
        ctx.arena
            .alloc_intersection(members.clone())
            .unwrap_or(members[0])
    };
    ctx.declare_value(symbol_id, &name.name, value_type);
    let spaces = ctx.symbol_capabilities.entry(symbol_id).or_default();
    spaces.has_type = true;
    spaces.has_value = true;
    spaces.value_available = true;
}

pub(in crate::bridge) fn constructor_type_params_for_overload(
    class: &Class<'_>,
    ctx: &mut CheckContext<'_, '_>,
) -> Vec<crate::types::FunctionTypeParam> {
    resolve_function_type_params(
        class.type_parameters.as_deref(),
        &mut ctx.namespace,
        &mut ctx.arena,
    )
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
pub(in crate::bridge) fn constructor_params_of<'a>(
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

pub(in crate::bridge) fn find_constructor<'a>(
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

// The constructor that has a body. With overloads, `find_constructor` finds the first signature,
// which has none; the implementation, the one whose body assigns the fields, comes last.
pub(in crate::bridge) fn find_constructor_body<'a>(
    class: &'a oxc_ast::ast::Class<'a>,
) -> Option<&'a oxc_ast::ast::FunctionBody<'a>> {
    use oxc_ast::ast::{ClassElement, MethodDefinitionKind};
    class.body.body.iter().find_map(|element| {
        let ClassElement::MethodDefinition(method) = element else {
            return None;
        };
        if method.kind != MethodDefinitionKind::Constructor {
            return None;
        }
        method.value.body.as_deref()
    })
}
