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
    let result = infer_arrow_function_type_inner(arrow, scoping, ctx);
    ctx.return_reads.owner = outer_owner;
    ctx.narrow = outer_narrow;
    result
}

fn infer_arrow_function_type_inner(
    arrow: &oxc_ast::ast::ArrowFunctionExpression,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> TypeId {
    let type_param_scope = ctx
        .namespace
        .push_decl_type_params(&mut ctx.arena, arrow.type_parameters.as_deref());
    let param_types =
        resolve_params_with_any_fallback(&arrow.params, &mut ctx.namespace, &mut ctx.arena);
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
            None => inferred,
        }
    } else {
        let ArrowFunctionBody::FunctionBody(body) = &arrow.body else {
            ctx.namespace.pop_type_params(type_param_scope);
            return ctx.arena.error();
        };
        // A block body has no return inference yet. Without an annotation its returns are
        // checked against any, so nothing false can be said about them.
        let return_type = declared_return.unwrap_or_else(|| ctx.arena.any());
        let return_scope = ctx.enter_function_return_scope(Some(return_type), false, arrow.r#async);
        for body_stmt in &body.statements {
            check_statement(body_stmt, scoping, ctx);
        }
        ctx.leave_return_scope(return_scope);
        return_type
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
    ctx: &mut CheckContext<'_, '_>,
) -> TypeId {
    // Narrowing from the body must not outlive the function; it is saved and
    // restored around the whole body, as in infer_arrow_function_type.
    let outer_narrow = ctx.narrow.clone();
    let visible = closure_view(&outer_narrow, func.span().start, scoping, ctx);
    ctx.narrow = visible;
    let result = infer_function_expression_type_inner(func, scoping, ctx);
    ctx.narrow = outer_narrow;
    result
}

fn infer_function_expression_type_inner(
    func: &Function,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> TypeId {
    let type_param_scope = ctx.namespace.push_type_params(&mut ctx.arena, func);
    // Consumed here, before anything else, so the request only ever applies to
    // this exact function and never to one nested inside its parameters or body.
    // A function with its own `this` parameter has a typed `this` and is never
    // implicit.
    let has_no_this =
        std::mem::take(&mut ctx.next_function_has_no_this) && func.this_param.is_none();
    let param_types =
        resolve_params_with_any_fallback(&func.params, &mut ctx.namespace, &mut ctx.arena);
    bind_params(&func.params, &param_types, scoping, ctx);

    let declared_return = func
        .return_type
        .as_ref()
        .and_then(|rt| resolve_type_annotation(rt, &mut ctx.namespace, &mut ctx.arena));
    let return_type = declared_return.unwrap_or_else(|| ctx.arena.any());

    if let Some(body) = &func.body {
        let return_scope = ctx.enter_function_return_scope(Some(return_type), false, func.r#async);

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
        ctx.leave_return_scope(return_scope);
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
