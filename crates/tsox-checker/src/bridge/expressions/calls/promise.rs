//! Promise-specific call forms: awaited types, Promise.resolve/all/race, executors.
use super::*;

// The value a promise settles to: `number` for `Promise<number>`, the type itself otherwise.
pub(in crate::bridge) fn awaited_type(ctx: &CheckContext<'_, '_>, mut type_id: TypeId) -> TypeId {
    for _ in 0..16 {
        match ctx.arena.promise_value_type(type_id) {
            Some(inner) => type_id = inner,
            None => break,
        }
    }
    type_id
}

// `Promise.resolve(v)`, `Promise.reject(e)`, `Promise.all([...])` and `Promise.race([...])` on the
// global Promise. The constructor object is not modelled, so these are typed here from what the
// library declares: `resolve<T>(v: T): Promise<Awaited<T>>`, `reject<T = never>(): Promise<T>`,
// `all` mapping a tuple of inputs to a tuple of what each settles to. Anything else on `Promise`
// (or a `Promise` the program declared itself) is left to the ordinary path.
pub(in crate::bridge) fn promise_static_call(
    call: &oxc_ast::ast::CallExpression,
    member: &oxc_ast::ast::StaticMemberExpression,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> Option<TypeId> {
    let Expression::Identifier(object) = &member.object else {
        return None;
    };
    if object.name != "Promise" || resolve_symbol_id(object, scoping).is_some() {
        return None;
    }
    let method = member.property.name.as_str();
    if !matches!(method, "resolve" | "reject" | "all" | "race" | "allSettled") {
        return None;
    }
    let mut argument_types = Vec::new();
    let mut array_elements: Option<Vec<TypeId>> = None;
    for argument in &call.arguments {
        let Some(expression) = argument.as_expression() else {
            return None;
        };
        if let Expression::ArrayExpression(array) = expression {
            let mut elements = Vec::new();
            for element in &array.elements {
                let element = element.as_expression()?;
                elements.push(infer_expression_type(element, scoping, ctx));
            }
            let tuple = ctx.arena.alloc_tuple(
                elements
                    .iter()
                    .map(|&type_id| crate::types::TupleElement {
                        type_id,
                        optional: false,
                        rest: false,
                    })
                    .collect(),
            );
            array_elements = Some(elements);
            argument_types.push(tuple);
        } else {
            argument_types.push(infer_expression_type(expression, scoping, ctx));
        }
    }
    match method {
        "resolve" => {
            let value = match argument_types.first() {
                Some(&first) => {
                    let first = awaited_type(ctx, first);
                    crate::types::widen(&ctx.arena, first)
                }
                None => ctx.arena.void(),
            };
            Some(ctx.arena.alloc_promise(value))
        }
        "reject" => {
            let never = ctx.arena.never();
            Some(ctx.arena.alloc_promise(never))
        }
        _ => {
            // An array that is not a literal (`Promise.all(promises)`): one element type
            // for all of it, so the result is an array of the awaited element.
            let Some(elements) = array_elements else {
                let &first = argument_types.first()?;
                let crate::types::Type::Array(element) = ctx.arena.get(first).clone() else {
                    return None;
                };
                let awaited = awaited_type(ctx, element);
                let inner = match method {
                    "race" => awaited,
                    "all" => ctx.arena.alloc(crate::types::Type::Array(awaited)),
                    _ => {
                        let fulfilled = settled_result(ctx, "fulfilled", "value", awaited);
                        let any = ctx.arena.any();
                        let rejected = settled_result(ctx, "rejected", "reason", any);
                        let result = ctx.arena.alloc_union(vec![fulfilled, rejected]);
                        ctx.arena.alloc(crate::types::Type::Array(result))
                    }
                };
                return Some(ctx.arena.alloc_promise(inner));
            };
            let settled: Vec<TypeId> = elements
                .iter()
                .map(|&element| {
                    let awaited = awaited_type(ctx, element);
                    crate::types::widen(&ctx.arena, awaited)
                })
                .collect();
            let inner = if method == "allSettled" {
                // Each input becomes `{ status: "fulfilled", value } | { status: "rejected", reason }`.
                let mut results = Vec::new();
                for &type_id in &settled {
                    let fulfilled = settled_result(ctx, "fulfilled", "value", type_id);
                    let any = ctx.arena.any();
                    let rejected = settled_result(ctx, "rejected", "reason", any);
                    results.push(ctx.arena.alloc_union(vec![fulfilled, rejected]));
                }
                ctx.arena.alloc_tuple(
                    results
                        .into_iter()
                        .map(|type_id| crate::types::TupleElement {
                            type_id,
                            optional: false,
                            rest: false,
                        })
                        .collect(),
                )
            } else if method == "all" {
                ctx.arena.alloc_tuple(
                    settled
                        .into_iter()
                        .map(|type_id| crate::types::TupleElement {
                            type_id,
                            optional: false,
                            rest: false,
                        })
                        .collect(),
                )
            } else {
                ctx.arena.alloc_union(settled)
            };
            Some(ctx.arena.alloc_promise(inner))
        }
    }
}

// `(resolve: (value: T | PromiseLike<T>) => void, reject: (reason?: any) => void) => void`,
// with `Promise<T>` standing for `PromiseLike<T>`. For `T` = `void` the value may be left out.
pub(in crate::bridge) fn promise_executor_type(
    ctx: &mut CheckContext<'_, '_>,
    value: TypeId,
) -> TypeId {
    use crate::types::{FunctionType, Param};
    let void = ctx.arena.void();
    let any = ctx.arena.any();
    let promise = ctx.arena.alloc_promise(value);
    let accepted = ctx.arena.alloc_union(vec![value, promise]);
    let param = |type_id, optional, name: &str| Param {
        type_id,
        optional,
        rest: false,
        name: Some(name.into()),
    };
    let optional_value = matches!(ctx.arena.get(value), Type::Void);
    let resolve = ctx.arena.alloc(Type::Function(FunctionType {
        params: vec![param(accepted, optional_value, "value")],
        return_type: void,
        type_params: Vec::new(),
        is_untyped: false,
    }));
    let reject = ctx.arena.alloc(Type::Function(FunctionType {
        params: vec![param(any, true, "reason")],
        return_type: void,
        type_params: Vec::new(),
        is_untyped: false,
    }));
    ctx.arena.alloc(Type::Function(FunctionType {
        params: vec![
            param(resolve, false, "resolve"),
            param(reject, true, "reject"),
        ],
        return_type: void,
        type_params: Vec::new(),
        is_untyped: false,
    }))
}
