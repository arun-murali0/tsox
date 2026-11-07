use oxc_ast::ast::LogicalOperator;
use oxc_semantic::Scoping;
use oxc_span::GetSpan;

use crate::arena::TypeId;
use crate::type_annotation::resolve_ts_type;

use super::super::context::CheckContext;
use super::super::narrow::{
    narrow_condition, narrow_to_falsy, narrow_to_non_nullish, narrow_to_truthy,
};
use super::infer_expression_type;

// TypeScript's result type for &&, ||, and ?? reflects that the operator only
// evaluates its right side under a specific condition on the left, so the result
// can be narrower than "either side's full type." a && b evaluates b only when a
// is truthy, so the result is (a's falsy slice) | (b's type); a || b evaluates b
// only when a is falsy, so the result is (a's truthy slice) | (b's type). Each
// arm below also narrows any variable in the left operand while checking the
// right operand, matching real short-circuit control flow, for example
// (x !== null && x.prop) should not flag x as possibly null inside .prop.
pub(super) fn infer_logical_expression_type(
    logical: &oxc_ast::ast::LogicalExpression,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> TypeId {
    match logical.operator {
        LogicalOperator::And => {
            let left_type = infer_expression_type(&logical.left, scoping, ctx);

            let (truthy_overrides, _) = narrow_condition(&logical.left, scoping, ctx);
            let outer_narrow = ctx.narrow.clone();
            ctx.narrow.extend(truthy_overrides);
            let right_type = infer_expression_type(&logical.right, scoping, ctx);
            ctx.narrow = outer_narrow;

            let left_falsy = narrow_to_falsy(&mut ctx.arena, left_type);
            ctx.arena.alloc_union(vec![left_falsy, right_type])
        }

        LogicalOperator::Or => {
            let left_type = infer_expression_type(&logical.left, scoping, ctx);

            let (_, falsy_overrides) = narrow_condition(&logical.left, scoping, ctx);
            let outer_narrow = ctx.narrow.clone();
            ctx.narrow.extend(falsy_overrides);
            let right_type = infer_expression_type(&logical.right, scoping, ctx);
            ctx.narrow = outer_narrow;

            let left_truthy = narrow_to_truthy(&mut ctx.arena, left_type);
            ctx.arena.alloc_union(vec![left_truthy, right_type])
        }

        LogicalOperator::Coalesce => {
            let left_type = infer_expression_type(&logical.left, scoping, ctx);
            let right_type = infer_expression_type(&logical.right, scoping, ctx);

            let left_non_nullish = narrow_to_non_nullish(&mut ctx.arena, left_type);
            ctx.arena.alloc_union(vec![left_non_nullish, right_type])
        }
    }
}

// A ternary's result is the union of both branches, and each branch is checked
// with the test's narrowing applied, the same way an if/else statement's two
// bodies are, so x !== null ? x.prop : "default" does not flag x as possibly
// null in the consequent branch.
pub(super) fn infer_conditional_expression_type(
    conditional: &oxc_ast::ast::ConditionalExpression,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> TypeId {
    infer_expression_type(&conditional.test, scoping, ctx);
    let (true_overrides, false_overrides) = narrow_condition(&conditional.test, scoping, ctx);
    let outer_narrow = ctx.narrow.clone();

    ctx.narrow.extend(true_overrides);
    let consequent_type = infer_expression_type(&conditional.consequent, scoping, ctx);
    ctx.narrow = outer_narrow.clone();

    ctx.narrow.extend(false_overrides);
    let alternate_type = infer_expression_type(&conditional.alternate, scoping, ctx);
    ctx.narrow = outer_narrow;

    ctx.conditional_arms
        .insert(conditional.span.start, (consequent_type, alternate_type));
    ctx.arena.alloc_union(vec![consequent_type, alternate_type])
}

// `as SomeType` always trusts the annotation, right or wrong; the original
// expression is still checked so its own independent problems are not hidden by
// the cast, but its inferred type never overrides what `as` declares. This
// matches how a type assertion is meant to be an escape hatch, not a checked
// conversion.
pub(super) fn infer_as_expression_type(
    as_expr: &oxc_ast::ast::TSAsExpression,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> TypeId {
    let is_const = is_const_assertion(&as_expr.type_annotation);
    let outer_const = ctx.const_context;
    ctx.const_context = outer_const || is_const;
    let inner = infer_expression_type(&as_expr.expression, scoping, ctx);
    ctx.const_context = outer_const;
    // Problem: `x as const` read `const` as a type name and reported "Cannot find name
    // 'const'". Picked: a const assertion keeps the expression's own type.
    // Cost: the result is not marked readonly (readonly is not modelled), so writes to it
    // are not reported; literals inside objects and tuples are kept.
    if is_const_assertion(&as_expr.type_annotation) {
        return inner;
    }
    resolve_ts_type(&as_expr.type_annotation, &mut ctx.namespace, &mut ctx.arena)
        .unwrap_or_else(|| ctx.arena.error())
}

// `expr satisfies T` checks the expression against T, with T as its contextual type, and
// keeps the expression's own type. A bad object literal is reported on the property that
// is wrong, like a declaration; otherwise on the whole expression (TS1360).
pub(super) fn infer_satisfies_expression_type(
    satisfies: &oxc_ast::ast::TSSatisfiesExpression,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> TypeId {
    let expected = resolve_ts_type(
        &satisfies.type_annotation,
        &mut ctx.namespace,
        &mut ctx.arena,
    );
    ctx.contextual = expected;
    let inner = infer_expression_type(&satisfies.expression, scoping, ctx);
    let Some(expected) = expected else {
        return inner;
    };
    if !ctx.semantic().is_assignable(inner, expected)
        && !super::elaborate::elaborate_mismatch(&satisfies.expression, inner, expected, ctx)
    {
        let message =
            crate::diagnostic_messages::messages::does_not_satisfy(&ctx.arena, inner, expected);
        ctx.error(message, satisfies.expression.span());
    }
    inner
}

fn is_const_assertion(annotation: &oxc_ast::ast::TSType) -> bool {
    matches!(
        annotation,
        oxc_ast::ast::TSType::TSTypeReference(reference)
            if matches!(&reference.type_name,
                oxc_ast::ast::TSTypeName::IdentifierReference(id) if id.name == "const")
    )
}

pub(super) fn infer_non_null_expression_type(
    non_null: &oxc_ast::ast::TSNonNullExpression,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> TypeId {
    let inner = infer_expression_type(&non_null.expression, scoping, ctx);
    narrow_to_non_nullish(&mut ctx.arena, inner)
}
