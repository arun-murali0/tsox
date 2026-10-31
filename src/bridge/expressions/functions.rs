use oxc_ast::ast::{ArrowFunctionBody, Function};
use oxc_semantic::Scoping;
use oxc_span::GetSpan;

use crate::arena::TypeId;
use crate::type_annotation::{
    resolve_function_type_params, resolve_params_with_any_fallback, resolve_type_annotation,
};
use crate::types::{FunctionType, Type};

use super::super::context::CheckContext;
use super::super::narrow::NarrowState;
use super::super::statements::{bind_params, check_statement};
use super::infer_expression_type;

// Contextual signatures supply types for unannotated callback parameters. Explicit
// annotations always win. Rest parameters use the contextual rest-array type when
// available; ordinary parameters use their positional type, including a rest element.
fn apply_contextual_parameter_types(
    params: &oxc_ast::ast::FormalParameters,
    resolved: &mut [crate::types::Param],
    contextual: Option<TypeId>,
    arena: &mut crate::arena::TypeArena,
) {
    let Some(contextual) = contextual else { return };
    let crate::types::Type::Function(signature) = arena.get(contextual).clone() else {
        return;
    };

    for (index, (syntax, resolved)) in params.items.iter().zip(resolved.iter_mut()).enumerate() {
        if syntax.type_annotation.is_some() {
            continue;
        }
        if let Some(type_id) = crate::subtyping::param_type_at(arena, &signature.params, index) {
            resolved.type_id = type_id;
        }
    }

    if let (Some(rest), Some(resolved_rest)) = (&params.rest, resolved.last_mut()) {
        if rest.type_annotation.is_none() {
            if let Some(contextual_rest) = signature.params.iter().find(|parameter| parameter.rest)
            {
                resolved_rest.type_id = contextual_rest.type_id;
            }
        }
    }
}

// What a closure body is allowed to keep of the narrowing where it was written.
//
// Problem: a closure used to see every narrowing in force at the point it was written, so
// `if (x === null) return; const g = () => x.length; x = null;` looked safe, though by the
// time g runs x is null again.
// Picked: tsc's rule. A `const` keeps its narrowing, and so does any other variable that is
// not assigned again after the closure is created (writes inside the closure count, as
// they are textually after its start). Property-path narrowing is never kept, since any
// call between creating and running the closure can change a property.
// Cost: a variable assigned in another closure is judged by position alone, and
// preserving a narrowing tsc would drop only hides an error, never invents one.
fn closure_view(
    outer: &NarrowState,
    closure_start: u32,
    scoping: &Scoping,
    ctx: &CheckContext<'_, '_>,
) -> NarrowState {
    outer.for_closure(|symbol_id| {
        scoping.symbol_flags(symbol_id).is_const_variable()
            || !ctx.writes.assigned_at_or_after(symbol_id, closure_start)
    })
}

// Arrow functions lexically inherit `this` from their enclosing scope in real
// JavaScript, so current_class_instance is deliberately left untouched here,
// unlike infer_function_expression_type below, which resets it.
pub(super) fn infer_arrow_function_type(
    arrow: &oxc_ast::ast::ArrowFunctionExpression,
    scoping: &Scoping,
    contextual: Option<TypeId>,
    ctx: &mut CheckContext<'_, '_>,
) -> TypeId {
    // Narrowing a guard clause establishes inside the body must not survive past
    // the function expression, and there is more than one exit path below, so
    // the save and restore wrap the whole body rather than each return. Not cleared:
    // an arrow still sees what the enclosing scope narrowed, as far as closure_view allows.
    let outer_narrow = ctx.narrow.clone();
    let visible = closure_view(&outer_narrow, arrow.span().start, scoping, ctx);
    ctx.narrow = visible;
    // An arrow's body is its own function: a call inside it does not read the return type
    // of the function it sits in, since the arrow's return type is only needed when the
    // arrow is called. An expression body has no return scope of its own to say so.
    let outer_owner = ctx.return_reads.owner.take();
    let result = infer_arrow_function_type_inner(arrow, scoping, contextual, ctx);
    ctx.return_reads.owner = outer_owner;
    ctx.narrow = outer_narrow;
    result
}

fn infer_arrow_function_type_inner(
    arrow: &oxc_ast::ast::ArrowFunctionExpression,
    scoping: &Scoping,
    contextual: Option<TypeId>,
    ctx: &mut CheckContext<'_, '_>,
) -> TypeId {
    let type_param_scope = ctx
        .namespace
        .push_decl_type_params(&mut ctx.arena, arrow.type_parameters.as_deref());
    let mut param_types =
        resolve_params_with_any_fallback(&arrow.params, &mut ctx.namespace, &mut ctx.arena);
    apply_contextual_parameter_types(&arrow.params, &mut param_types, contextual, &mut ctx.arena);
    bind_params(&arrow.params, &param_types, scoping, ctx);

    let declared_return = arrow
        .return_type
        .as_ref()
        .and_then(|rt| resolve_type_annotation(rt, &mut ctx.namespace, &mut ctx.arena));

    let return_type = if let Some(body_expr) = arrow.body.as_expression() {
        let inferred = infer_expression_type(body_expr, scoping, ctx);
        match declared_return {
            Some(declared) => {
                // An async arrow's expression is a T where the annotation says Promise<T>.
                let (checked, expected) = if arrow.r#async {
                    (ctx.awaited_type(inferred), ctx.promised_type(declared))
                } else {
                    (inferred, declared)
                };
                if !ctx.is_assignable_instantiating(checked, expected) {
                    ctx.error(
                        crate::diagnostic_messages::messages::return_type_mismatch(
                            &ctx.arena, checked, expected,
                        ),
                        body_expr.span(),
                    );
                }
                declared
            }
            None if arrow.r#async => {
                let awaited = ctx.awaited_type(inferred);
                ctx.arena.alloc_promise(awaited)
            }
            None => inferred,
        }
    } else {
        let ArrowFunctionBody::FunctionBody(body) = &arrow.body else {
            ctx.namespace.pop_type_params(type_param_scope);
            return ctx.arena.error();
        };
        // Block-bodied arrows infer from their return statements just like function
        // declarations. Leaving this as `any` erases the callback's evidence from a
        // generic call, so `use(1, x => { return x.length })` could appear to return
        // anything and invalid assignments would be accepted.
        let infers_return = declared_return.is_none();
        let placeholder = declared_return.unwrap_or_else(|| ctx.arena.any());
        let return_scope =
            ctx.enter_function_return_scope(Some(placeholder), infers_return, arrow.r#async);
        for body_stmt in &body.statements {
            check_statement(body_stmt, scoping, ctx);
        }
        let returned = ctx.leave_return_scope(return_scope);
        if infers_return {
            let inferred = if returned.is_empty() {
                ctx.arena.void()
            } else {
                ctx.arena.alloc_union(returned)
            };
            if arrow.r#async {
                let awaited = ctx.awaited_type(inferred);
                ctx.arena.alloc_promise(awaited)
            } else {
                inferred
            }
        } else {
            placeholder
        }
    };

    let type_params = resolve_function_type_params(
        arrow.type_parameters.as_deref(),
        &mut ctx.namespace,
        &mut ctx.arena,
    );
    ctx.namespace.pop_type_params(type_param_scope);
    ctx.arena.alloc(Type::Function(FunctionType {
        params: param_types,
        return_type,
        type_params,
        is_untyped: false,
    }))
}

pub(super) fn infer_function_expression_type(
    func: &Function,
    scoping: &Scoping,
    contextual: Option<TypeId>,
    ctx: &mut CheckContext<'_, '_>,
) -> TypeId {
    // Narrowing from the body must not outlive the function; it is saved and
    // restored around the whole body, as in infer_arrow_function_type.
    let outer_narrow = ctx.narrow.clone();
    let visible = closure_view(&outer_narrow, func.span().start, scoping, ctx);
    ctx.narrow = visible;
    let result = infer_function_expression_type_inner(func, scoping, contextual, ctx);
    ctx.narrow = outer_narrow;
    result
}

fn infer_function_expression_type_inner(
    func: &Function,
    scoping: &Scoping,
    contextual: Option<TypeId>,
    ctx: &mut CheckContext<'_, '_>,
) -> TypeId {
    let type_param_scope = ctx.namespace.push_type_params(&mut ctx.arena, func);
    // Consumed here, before anything else, so the request only ever applies to
    // this exact function and never to one nested inside its parameters or body.
    // A function with its own `this` parameter has a typed `this` and is never
    // implicit.
    let has_no_this =
        std::mem::take(&mut ctx.next_function_has_no_this) && func.this_param.is_none();
    let mut param_types =
        resolve_params_with_any_fallback(&func.params, &mut ctx.namespace, &mut ctx.arena);
    apply_contextual_parameter_types(&func.params, &mut param_types, contextual, &mut ctx.arena);
    bind_params(&func.params, &param_types, scoping, ctx);

    let declared_return = func
        .return_type
        .as_ref()
        .and_then(|rt| resolve_type_annotation(rt, &mut ctx.namespace, &mut ctx.arena));
    // Generator return semantics are iterator-based and are not modeled by this
    // expression path yet. Keep their previous `any` recovery rather than treating
    // yielded values as ordinary function returns.
    let infers_return = declared_return.is_none() && !func.generator;
    let placeholder = declared_return.unwrap_or_else(|| ctx.arena.any());
    let mut return_type = placeholder;

    if let Some(body) = &func.body {
        let return_scope =
            ctx.enter_function_return_scope(Some(placeholder), infers_return, func.r#async);

        // Unlike an arrow function, a plain function expression gets its own
        // `this` binding at call time rather than inheriting the enclosing one,
        // so any class-instance context from an outer method body must not leak
        // into this function's body.
        let outer_class_instance = ctx.current_class_instance.take();
        let outer_implicit_this = std::mem::replace(&mut ctx.implicit_this, has_no_this);
        for body_stmt in &body.statements {
            check_statement(body_stmt, scoping, ctx);
        }
        ctx.implicit_this = outer_implicit_this;
        ctx.current_class_instance = outer_class_instance;
        let returned = ctx.leave_return_scope(return_scope);
        if infers_return {
            return_type = if returned.is_empty() {
                ctx.arena.void()
            } else {
                ctx.arena.alloc_union(returned)
            };
            if func.r#async {
                let awaited = ctx.awaited_type(return_type);
                return_type = ctx.arena.alloc_promise(awaited);
            }
        }
    }

    let type_params = resolve_function_type_params(
        func.type_parameters.as_deref(),
        &mut ctx.namespace,
        &mut ctx.arena,
    );
    ctx.namespace.pop_type_params(type_param_scope);
    ctx.arena.alloc(Type::Function(FunctionType {
        params: param_types,
        return_type,
        type_params,
        is_untyped: false,
    }))
}
