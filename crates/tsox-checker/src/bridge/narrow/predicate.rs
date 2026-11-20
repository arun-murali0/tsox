// Owns: narrowing by a user-defined type predicate call (`x is T`, `asserts x is T`).

use super::*;

// What a call to a function that says `x is T` (or `asserts x is T`, or `asserts x`) tells
// about its argument: the pair of overlays for the call being true or false. The argument
// can be a variable, or a property of one (`animal.type`), which narrows the variable by
// that property like a comparison with a literal does.
//
// `allow_asserts` picks which kind of predicate counts: a condition (`if (isFoo(x))`) takes
// `x is T`, a statement (`assertFoo(x);`) takes the `asserts` kind.
pub fn narrow_by_predicate_call(
    call: &oxc_ast::ast::CallExpression,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
    allow_asserts: bool,
) -> Option<(NarrowState, NarrowState)> {
    let Expression::Identifier(callee) = &call.callee else {
        return None;
    };
    let symbol_id = resolve_symbol_id(callee, scoping)?;
    let predicate = *ctx.type_predicates.get(&symbol_id)?;
    if predicate.asserts != allow_asserts {
        return None;
    }
    let argument = call.arguments.get(predicate.param_index)?.as_expression()?;
    let Some(claimed) = predicate.type_id else {
        // `asserts x`: the argument is truthy afterwards.
        return Some(narrow_condition(argument, scoping, ctx));
    };
    let claimed = claimed_type(call, symbol_id, claimed, scoping, ctx)?;

    let mut argument = argument;
    while let Expression::ParenthesizedExpression(inner) = argument {
        argument = &inner.expression;
    }
    // `a?.p` is a chain around the member access.
    let chained_member = match argument {
        Expression::ChainExpression(chain) => match &chain.expression {
            oxc_ast::ast::ChainElement::StaticMemberExpression(member) => Some(member),
            _ => None,
        },
        _ => None,
    };
    match argument {
        Expression::Identifier(ident) => {
            let symbol = resolve_symbol_id(ident, scoping)?;
            Some(by_symbol(ctx, symbol, move |arena, current, want_true| {
                narrow_to_claimed(arena, current, claimed, want_true)
            }))
        }
        _ if chained_member.is_some()
            || matches!(argument, Expression::StaticMemberExpression(_)) =>
        {
            let member = match chained_member {
                Some(member) => member,
                None => match argument {
                    Expression::StaticMemberExpression(member) => member,
                    _ => return None,
                },
            };
            let Expression::Identifier(ident) = &member.object else {
                return None;
            };
            let symbol = resolve_symbol_id(ident, scoping)?;
            let literals = union_members(&ctx.arena, claimed).unwrap_or_else(|| vec![claimed]);
            if !literals.iter().all(|&l| {
                matches!(
                    ctx.arena.get(l),
                    Type::StringLiteral(_) | Type::NumberLiteral(_) | Type::BooleanLiteral(_)
                )
            }) {
                return None;
            }
            let property = member.property.name.to_string();
            let optional = member.optional;
            Some(by_symbol(ctx, symbol, move |arena, current, want_true| {
                // `a?.p` equal to a literal means `a` is there.
                let base = if optional && want_true {
                    narrow_to_non_nullish(arena, current)
                } else {
                    current
                };
                narrow_by_property_literals(arena, base, &property, &literals, want_true)
            }))
        }
        _ => None,
    }
}

// The `T` of a predicate. A bare type parameter of the function (`asserts value is T` in
// `<T>(value: any, type: T)`) is whatever the other argument of that type is.
pub(super) fn claimed_type(
    call: &oxc_ast::ast::CallExpression,
    callee: SymbolId,
    claimed: TypeId,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> Option<TypeId> {
    if !matches!(ctx.arena.get(claimed), Type::GenericParameter(..)) {
        if !crate::semantic::GenericSolver::contains_type_param(&ctx.arena, claimed) {
            return Some(claimed);
        }
        // `target is { [K in P]: unknown }`: the claim is written in the callee's type
        // parameters. Each one that is the type of a parameter is read from the argument
        // given for it, and the claim is instantiated with those.
        let signature = ctx.symbols.get(callee)?;
        let Type::Function(function) = ctx.arena.get(signature).clone() else {
            return Some(claimed);
        };
        let mut bindings = Vec::new();
        for type_param in &function.type_params {
            let Some(index) = function.params.iter().position(|p| {
                matches!(ctx.arena.get(p.type_id), Type::GenericParameter(id, _, _) if *id == type_param.id)
            }) else {
                continue;
            };
            let Some(argument) = call.arguments.get(index).and_then(|a| a.as_expression()) else {
                continue;
            };
            let before = ctx.diagnostics.len();
            let inferred =
                crate::bridge::expressions::infer_expression_type(argument, scoping, ctx);
            ctx.diagnostics.truncate(before);
            bindings.push((type_param.id, inferred));
        }
        let instantiated = crate::semantic::GenericSolver::substitute_bound_type_params(
            &mut ctx.arena,
            claimed,
            &bindings,
        );
        return Some(instantiated);
    }
    let signature = ctx.symbols.get(callee)?;
    let Type::Function(function) = ctx.arena.get(signature).clone() else {
        return None;
    };
    let index = function.params.iter().position(|p| p.type_id == claimed)?;
    let argument = call.arguments.get(index)?.as_expression()?;
    // The argument was inferred once already, with its diagnostics; inferring it again
    // must not report them a second time.
    let before = ctx.diagnostics.len();
    let inferred = crate::bridge::expressions::infer_expression_type(argument, scoping, ctx);
    ctx.diagnostics.truncate(before);
    Some(inferred)
}

pub(super) fn narrow_to_claimed(
    arena: &mut TypeArena,
    current: TypeId,
    claimed: TypeId,
    want_true: bool,
) -> TypeId {
    // A claim still written in the callee's own type parameters (`target is { [K in P]: ... }`)
    // was not instantiated with the call's arguments, and narrowing to it would put `P` into
    // the caller's types: the value keeps its type.
    if crate::semantic::GenericSolver::contains_type_param(arena, claimed) {
        return current;
    }
    let members = union_members(arena, current).unwrap_or_else(|| vec![current]);
    let mut kept = Vec::with_capacity(members.len());
    for member in &members {
        let fits = crate::subtyping::is_subtype(arena, *member, claimed);
        if fits == want_true {
            kept.push(*member);
        }
    }
    if want_true && kept.is_empty() {
        // Nothing of `current` is `claimed`: it is `claimed` itself when that is narrower,
        // and the two together otherwise.
        if crate::subtyping::is_subtype(arena, claimed, current) {
            return claimed;
        }
        return arena
            .alloc_intersection(vec![current, claimed])
            .unwrap_or(claimed);
    }
    arena.alloc_union(kept)
}
