//! The condition shapes that narrow a plain variable, as a first-match table. Each narrower looks
//! at the test expression, answers `Some((when_true, when_false))` for the shape it owns and
//! `None` for any other, so adding a shape is one function and one row. Order matters only where
//! two rows could both accept a node: the aliased-condition row comes first because it keys on an
//! identifier that the plain identifier row would also accept.
use super::*;

pub(super) type NarrowerFn = for<'a, 'b, 'c> fn(
    &Expression<'a>,
    &Scoping,
    &mut CheckContext<'b, 'c>,
) -> Option<(NarrowState, NarrowState)>;

#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct Narrower {
    pub name: &'static str,
    pub doc: &'static str,
    pub run: NarrowerFn,
}

pub(super) const NARROWERS: &[Narrower] = &[
    Narrower {
        name: "aliased_condition",
        doc: "A `const` holding a condition stands for that condition.",
        run: aliased_condition,
    },
    Narrower {
        name: "parenthesized",
        doc: "oxc keeps parentheses as a node; narrow what they wrap.",
        run: parenthesized,
    },
    Narrower {
        name: "optional_chain_truthy",
        doc: "`if (o?.foo)`: truthy means the chain got past `o`.",
        run: optional_chain_truthy,
    },
    Narrower {
        name: "binary",
        doc: "`in`, `instanceof` and the four equality operators.",
        run: binary,
    },
    Narrower {
        name: "logical",
        doc: "`a && b` and `a || b` compose the pairs of their sides.",
        run: logical,
    },
    Narrower {
        name: "not",
        doc: "`!x` swaps the pair of its operand.",
        run: not,
    },
    Narrower {
        name: "truthy_identifier",
        doc: "`if (x)` narrows x by truthiness.",
        run: truthy_identifier,
    },
    Narrower {
        name: "predicate_call",
        doc: "`if (isFoo(x))`: a user-defined type guard.",
        run: predicate_call,
    },
    Narrower {
        name: "member_truthiness",
        doc: "`if (result.ok)`: a boolean discriminant tested without a comparison.",
        run: member_truthiness,
    },
];

fn aliased_condition(
    test: &Expression,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> Option<(NarrowState, NarrowState)> {
    let Expression::Identifier(id) = test else {
        return None;
    };
    let reference = id.reference_id.get()?;
    let symbol_id = scoping.get_reference(reference).symbol_id()?;
    ctx.aliased_conditions.get(&symbol_id).cloned()
}

fn parenthesized(
    test: &Expression,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> Option<(NarrowState, NarrowState)> {
    let Expression::ParenthesizedExpression(inner) = test else {
        return None;
    };
    Some(narrow_condition(&inner.expression, scoping, ctx))
}

fn non_nullish_when_true(
    ctx: &mut CheckContext<'_, '_>,
    symbol_id: SymbolId,
) -> (NarrowState, NarrowState) {
    by_symbol(ctx, symbol_id, |arena, current, want_true| {
        if want_true {
            narrow_to_non_nullish(arena, current)
        } else {
            current
        }
    })
}

fn optional_chain_truthy(
    test: &Expression,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> Option<(NarrowState, NarrowState)> {
    if !matches!(test, Expression::ChainExpression(_)) {
        return None;
    }
    let symbol_id = chain_root(test, scoping)?;
    Some(non_nullish_when_true(ctx, symbol_id))
}

fn binary(
    test: &Expression,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> Option<(NarrowState, NarrowState)> {
    let Expression::BinaryExpression(bin) = test else {
        return None;
    };
    // `in` and `instanceof` are not equality at all, so they are handled before the equality
    // detection, which would find nothing to match.
    match bin.operator {
        BinaryOperator::In => return Some(narrow_by_in(bin, scoping, ctx)),
        BinaryOperator::Instanceof => {
            let mut pair = narrow_by_instanceof(bin, scoping, ctx);
            // `o?.baz instanceof Error` holding means the chain got past `o`.
            if let Some(symbol_id) = chain_root(skip_parens_expr(&bin.left), scoping) {
                let (when_true, _) = non_nullish_when_true(ctx, symbol_id);
                pair.0.extend(when_true);
            }
            return Some(pair);
        }
        _ => {}
    }
    // `==` and `!=` are treated like `===` and `!==`, flipping which branch gets which side.
    let negated = matches!(
        bin.operator,
        BinaryOperator::Inequality | BinaryOperator::StrictInequality
    );
    let is_equality = matches!(
        bin.operator,
        BinaryOperator::Equality
            | BinaryOperator::Inequality
            | BinaryOperator::StrictEquality
            | BinaryOperator::StrictInequality
    );
    if !is_equality {
        return Some(empty_pair());
    }
    Some(equality::narrow_equality(bin, negated, scoping, ctx))
}

// `a && b`: the true side needs both to hold; the false side is reached either because `a` was
// false or because `a` held and `b` did not, so it is the join of those two paths. `b` is
// narrowed against the state `a` establishes, since it only runs when `a` held. `a || b` is the
// mirror image. `??` narrows nothing here.
fn logical(
    test: &Expression,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> Option<(NarrowState, NarrowState)> {
    let Expression::LogicalExpression(logical) = test else {
        return None;
    };
    Some(match logical.operator {
        LogicalOperator::And => {
            let (left_true, left_false) = narrow_condition(&logical.left, scoping, ctx);
            let outer_narrow = ctx.narrow.clone();
            ctx.narrow.extend(left_true.clone());
            let (right_true, right_false) = narrow_condition(&logical.right, scoping, ctx);
            ctx.narrow = outer_narrow;

            let mut true_overlay = left_true.clone();
            true_overlay.extend(right_true);
            let mut right_false_path = left_true;
            right_false_path.extend(right_false);
            let false_overlay = join_states(&left_false, &right_false_path, &mut ctx.arena);
            (true_overlay, false_overlay)
        }
        LogicalOperator::Or => {
            let (left_true, left_false) = narrow_condition(&logical.left, scoping, ctx);
            let outer_narrow = ctx.narrow.clone();
            ctx.narrow.extend(left_false.clone());
            let (right_true, right_false) = narrow_condition(&logical.right, scoping, ctx);
            ctx.narrow = outer_narrow;

            let mut false_overlay = left_false.clone();
            false_overlay.extend(right_false);
            let mut right_true_path = left_false;
            right_true_path.extend(right_true);
            let true_overlay = join_states(&left_true, &right_true_path, &mut ctx.arena);
            (true_overlay, false_overlay)
        }
        LogicalOperator::Coalesce => empty_pair(),
    })
}

fn not(
    test: &Expression,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> Option<(NarrowState, NarrowState)> {
    let Expression::UnaryExpression(unary) = test else {
        return None;
    };
    if unary.operator != UnaryOperator::LogicalNot {
        return None;
    }
    let (true_overrides, false_overrides) = narrow_condition(&unary.argument, scoping, ctx);
    Some((false_overrides, true_overrides))
}

fn truthy_identifier(
    test: &Expression,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> Option<(NarrowState, NarrowState)> {
    let Expression::Identifier(ident) = test else {
        return None;
    };
    let Some(symbol_id) = resolve_symbol_id(ident, scoping) else {
        return Some(empty_pair());
    };
    Some(by_symbol(ctx, symbol_id, narrow_truthy))
}

fn predicate_call(
    test: &Expression,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> Option<(NarrowState, NarrowState)> {
    let Expression::CallExpression(call) = test else {
        return None;
    };
    Some(narrow_by_predicate_call(call, scoping, ctx, false).unwrap_or_else(empty_pair))
}

fn member_truthiness(
    test: &Expression,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> Option<(NarrowState, NarrowState)> {
    let Expression::StaticMemberExpression(member) = test else {
        return None;
    };
    if member.optional {
        return None;
    }
    let Expression::Identifier(ident) = &member.object else {
        return Some(empty_pair());
    };
    let Some(symbol_id) = resolve_symbol_id(ident, scoping) else {
        return Some(empty_pair());
    };
    let property = member.property.name.to_string();
    Some(by_symbol(
        ctx,
        symbol_id,
        move |arena, current, want_truthy| {
            narrow_by_property_truthiness(arena, current, &property, want_truthy)
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn narrowers_have_unique_names_and_docs() {
        let mut seen = std::collections::HashSet::new();
        for narrower in NARROWERS {
            assert!(
                seen.insert(narrower.name),
                "duplicate narrower {}",
                narrower.name
            );
            assert!(!narrower.doc.is_empty());
        }
    }
}
