use oxc_ast::ast::{BinaryOperator, Expression};
use oxc_span::{GetSpan, Span};

use crate::arena::TypeId;
use crate::types::Type;

use super::super::narrow::narrow_to_non_nullish;

use super::super::context::CheckContext;

// tsc reports an operand that may be null or undefined where the operator needs a
// number once, on the operand, and then goes on with the operand's type without them,
// so `a + 1` with `a: number | undefined` is that one error and not an operator
// mismatch. Applies when the other operand is not a string (`"s" + a` concatenates) and
// what is left of the operand is a number (or, for a comparison, a string).
//
// Returns the operand types to carry on with.
pub(super) fn strip_possibly_nullish(
    operator: BinaryOperator,
    operands: [(&Expression, TypeId); 2],
    ctx: &mut CheckContext<'_, '_>,
) -> (TypeId, TypeId) {
    let relational = matches!(
        operator,
        BinaryOperator::LessThan
            | BinaryOperator::LessEqualThan
            | BinaryOperator::GreaterThan
            | BinaryOperator::GreaterEqualThan
    );
    let numeric = relational
        || matches!(
            operator,
            BinaryOperator::Addition
                | BinaryOperator::Subtraction
                | BinaryOperator::Multiplication
                | BinaryOperator::Division
                | BinaryOperator::Remainder
                | BinaryOperator::Exponential
        );
    if !numeric {
        return (operands[0].1, operands[1].1);
    }

    let string_ty = ctx.arena.string();
    let number_ty = ctx.arena.number();
    let is_string = |ctx: &mut CheckContext<'_, '_>, ty: TypeId| {
        ty != ctx.arena.any() && ctx.semantic().is_assignable(ty, string_ty)
    };
    let sides_are_strings = [is_string(ctx, operands[0].1), is_string(ctx, operands[1].1)];

    let mut result = [operands[0].1, operands[1].1];
    for (index, (expr, ty)) in operands.into_iter().enumerate() {
        // `+` with a string on the other side is concatenation, which accepts anything.
        if operator == BinaryOperator::Addition && sides_are_strings[1 - index] {
            continue;
        }
        let (has_null, has_undefined) = nullish_members(ctx, ty);
        if !has_null && !has_undefined {
            continue;
        }
        let rest = narrow_to_non_nullish(&mut ctx.arena, ty);
        let fits = ctx.semantic().is_assignable(rest, number_ty)
            || (relational && ctx.semantic().is_assignable(rest, string_ty));
        if !fits {
            continue;
        }
        let name = entity_name(expr);
        ctx.error(
            crate::diagnostic_messages::messages::possibly_nullish(
                name.as_deref(),
                has_null,
                has_undefined,
            ),
            expr.span(),
        );
        result[index] = rest;
    }
    (result[0], result[1])
}

pub(super) fn nullish_members(ctx: &CheckContext<'_, '_>, ty: TypeId) -> (bool, bool) {
    let members = match ctx.arena.get(ty) {
        Type::Union(members) => members.clone(),
        _ => vec![ty],
    };
    let has = |wanted: fn(&Type) -> bool| members.iter().any(|&m| wanted(ctx.arena.get(m)));
    (
        has(|t| matches!(t, Type::Null)),
        has(|t| matches!(t, Type::Undefined)),
    )
}

// The source text tsc names an operand by: a plain name, or a chain of property
// accesses on one (`o.v`, `this.items`). Anything else (a call, an index) has none.
pub(super) fn entity_name(expr: &Expression) -> Option<String> {
    match expr {
        Expression::Identifier(id) => Some(id.name.to_string()),
        Expression::ThisExpression(_) => Some("this".to_string()),
        Expression::ParenthesizedExpression(inner) => entity_name(&inner.expression),
        Expression::StaticMemberExpression(member) if !member.optional => Some(format!(
            "{}.{}",
            entity_name(&member.object)?,
            member.property.name
        )),
        _ => None,
    }
}

// TS2367: the two sides of `===` (or `==`, `!==`, `!=`) can never be equal. Only the clear cases
// are reported: primitives of different kinds, two different literals, or a primitive against an
// object, array or tuple. Unions, generics, enums and anything unmodelled are left alone.
// The literal types a type is made of, when it is one literal or a union of only literals.
fn literal_members(ctx: &CheckContext<'_, '_>, type_id: TypeId) -> Option<Vec<TypeId>> {
    let is_literal = |id: TypeId| {
        matches!(
            ctx.arena.get(id),
            Type::StringLiteral(_) | Type::NumberLiteral(_) | Type::BooleanLiteral(_)
        )
    };
    match ctx.arena.get(type_id) {
        Type::Union(members) if members.iter().all(|&m| is_literal(m)) => Some(members.to_vec()),
        _ if is_literal(type_id) => Some(vec![type_id]),
        _ => None,
    }
}

fn comparison_has_no_overlap(ctx: &CheckContext<'_, '_>, left: TypeId, right: TypeId) -> bool {
    // Two sets of literals with no member in common (`"a" | "b"` against `"c"`).
    if let (Some(left_members), Some(right_members)) =
        (literal_members(ctx, left), literal_members(ctx, right))
    {
        return !left_members.iter().any(|l| {
            right_members
                .iter()
                .any(|r| ctx.arena.get(*l) == ctx.arena.get(*r))
        });
    }
    #[derive(PartialEq, Clone, Copy)]
    enum Kind {
        String,
        Number,
        Boolean,
        Composite,
        Other,
    }
    let kind_of = |type_id: TypeId| match ctx.arena.get(type_id) {
        Type::String | Type::StringLiteral(_) => Kind::String,
        Type::Number | Type::NumberLiteral(_) => Kind::Number,
        Type::Boolean | Type::BooleanLiteral(_) => Kind::Boolean,
        Type::Object(object)
            if object.properties.iter().any(|p| !p.optional) && object.string_index.is_none() =>
        {
            Kind::Composite
        }
        Type::Array(_) | Type::Tuple(_) => Kind::Composite,
        _ => Kind::Other,
    };
    let (left_kind, right_kind) = (kind_of(left), kind_of(right));
    if left_kind == Kind::Other || right_kind == Kind::Other {
        return false;
    }
    if left_kind == Kind::Composite && right_kind == Kind::Composite {
        return false;
    }
    if left_kind != right_kind {
        return true;
    }
    // The same primitive kind: only two different literals cannot meet.
    match (ctx.arena.get(left), ctx.arena.get(right)) {
        (Type::StringLiteral(a), Type::StringLiteral(b)) => a != b,
        (Type::NumberLiteral(a), Type::NumberLiteral(b)) => a != b,
        (Type::BooleanLiteral(a), Type::BooleanLiteral(b)) => a != b,
        _ => false,
    }
}

pub(super) fn infer_binary_expression_type(
    operator: BinaryOperator,
    left: TypeId,
    right: TypeId,
    span: Span,
    ctx: &mut CheckContext<'_, '_>,
) -> TypeId {
    match operator {
        BinaryOperator::Equality
        | BinaryOperator::Inequality
        | BinaryOperator::StrictEquality
        | BinaryOperator::StrictInequality => {
            if comparison_has_no_overlap(ctx, left, right) {
                // tsc names the literals when both sides are literal types, and otherwise the
                // primitives they belong to.
                let literal = |ctx: &CheckContext<'_, '_>, type_id: TypeId| {
                    literal_members(ctx, type_id).is_some()
                };
                let (shown_left, shown_right) = if literal(ctx, left) && literal(ctx, right) {
                    (left, right)
                } else {
                    (
                        crate::types::widen(&ctx.arena, left),
                        crate::types::widen(&ctx.arena, right),
                    )
                };
                ctx.error(
                    crate::diagnostic_messages::messages::comparison_no_overlap(
                        &ctx.arena,
                        shown_left,
                        shown_right,
                    ),
                    span,
                );
            }
            ctx.arena.boolean()
        }

        BinaryOperator::LessThan
        | BinaryOperator::LessEqualThan
        | BinaryOperator::GreaterThan
        | BinaryOperator::GreaterEqualThan => ctx.arena.boolean(),

        // JavaScript's + is overloaded: string concatenation if either side could
        // be a string, otherwise numeric addition if both sides could be numbers.
        // The string check is tried first since that matches runtime semantics,
        // where "1" + 1 is string concatenation, not addition.
        BinaryOperator::Addition => {
            // Read before calling ctx.semantic(): SemanticQueries borrows
            // ctx.arena and ctx.relation_cache together for as long as it lives,
            // so ctx.arena can't be reached again (even just to read a fixed
            // primitive id) while a SemanticQueries value from an earlier call
            // in this same expression is still alive.
            // Problem: after `total + x.length` reported `x` as possibly null, `x.length`
            // was the error type, which is assignable to string, so the sum counted as a
            // string concatenation and `total = ...` reported a second, unrelated
            // mismatch.
            // Picked: an operand that is already an error makes the sum an error too,
            // which every later check treats as silent, so one mistake reports once.
            let error_ty = ctx.arena.error();
            if left == error_ty || right == error_ty {
                return error_ty;
            }
            let any_ty = ctx.arena.any();
            if left == any_ty || right == any_ty {
                // Checked before the string/number rules below, not after: `any`
                // is a subtype of every type, string included, so `is_string`
                // would otherwise already be true for `any + 1` and wrongly win,
                // reporting `string` instead of the `any` real TypeScript infers.
                return ctx.arena.any();
            }
            let string_ty = ctx.arena.string();
            let number_ty = ctx.arena.number();
            let is_string = ctx.semantic().is_assignable(left, string_ty)
                || ctx.semantic().is_assignable(right, string_ty);
            let is_number = ctx.semantic().is_assignable(left, number_ty)
                && ctx.semantic().is_assignable(right, number_ty);
            if is_string {
                ctx.arena.string()
            } else if is_number {
                ctx.arena.number()
            } else {
                push_binary_op_mismatch(ctx, span, "+", left, right);
                ctx.arena.error()
            }
        }

        BinaryOperator::Subtraction
        | BinaryOperator::Multiplication
        | BinaryOperator::Division
        | BinaryOperator::Remainder
        | BinaryOperator::Exponential => {
            // No `any` special-case here, unlike Addition above: verified
            // against real tsc (see
            // tests/fixtures/misc-fixes/any_minus_number_infers_number_not_any.ts),
            // `any - 1` actually infers `number`, not `any`. Only `+` propagates
            // `any` in real TypeScript, because of its ambiguity between the
            // string and number overloads when one side is `any`; every other
            // arithmetic operator only has a (number, number) => number
            // overload, and `any` simply satisfies that parameter type the same
            // way it satisfies any other, giving back the overload's own
            // `number` result rather than `any`.
            let number_ty = ctx.arena.number();
            let left_ok = ctx.semantic().is_assignable(left, number_ty);
            let right_ok = ctx.semantic().is_assignable(right, number_ty);
            if left_ok && right_ok {
                ctx.arena.number()
            } else {
                // One diagnostic per bad side, as tsc reports it (TS2362 and
                // TS2363). Both land on the whole expression's span, since
                // only that is available here.
                if !left_ok {
                    ctx.error(
                        crate::diagnostic_messages::messages::arithmetic_left_operand_invalid(),
                        span,
                    );
                }
                if !right_ok {
                    ctx.error(
                        crate::diagnostic_messages::messages::arithmetic_right_operand_invalid(),
                        span,
                    );
                }
                ctx.arena.error()
            }
        }

        // Shifts and bitwise operators hold their operands to the same rule the
        // arithmetic group above does: tsc runs them through the same check (the same
        // TS2362 and TS2363), and they produce a number.
        BinaryOperator::ShiftLeft
        | BinaryOperator::ShiftRight
        | BinaryOperator::ShiftRightZeroFill
        | BinaryOperator::BitwiseAnd
        | BinaryOperator::BitwiseOR
        | BinaryOperator::BitwiseXOR => {
            // An operand that is already an error makes the result one too, so one
            // mistake reports once, as the Addition arm does.
            let error_ty = ctx.arena.error();
            if left == error_ty || right == error_ty {
                return error_ty;
            }
            // tsc has its own code for an `unknown` operand (TS18046), which this
            // checker does not model; such an operand stays silent rather than being
            // reported under the arithmetic code.
            if may_be_unknown(ctx, left) || may_be_unknown(ctx, right) {
                return ctx.arena.number();
            }
            let number_ty = ctx.arena.number();
            let left_ok = ctx.semantic().is_assignable(left, number_ty);
            let right_ok = ctx.semantic().is_assignable(right, number_ty);
            if left_ok && right_ok {
                ctx.arena.number()
            } else {
                // One diagnostic per bad side, as tsc reports it (TS2362 and
                // TS2363). Both land on the whole expression's span, since
                // only that is available here.
                if !left_ok {
                    ctx.error(
                        crate::diagnostic_messages::messages::arithmetic_left_operand_invalid(),
                        span,
                    );
                }
                if !right_ok {
                    ctx.error(
                        crate::diagnostic_messages::messages::arithmetic_right_operand_invalid(),
                        span,
                    );
                }
                ctx.arena.error()
            }
        }

        _ => ctx.arena.error(),
    }
}

// Whether `unknown` is the type or one of a union's members.
fn may_be_unknown(ctx: &CheckContext<'_, '_>, id: TypeId) -> bool {
    match ctx.arena.get(id) {
        Type::Unknown => true,
        Type::Union(members) => members
            .iter()
            .any(|&member| matches!(ctx.arena.get(member), Type::Unknown)),
        _ => false,
    }
}

// Both mismatch sites above (`+` and the arithmetic group) need the same
// diagnostic shape and differ only in which operator string to name, so it's
// pulled out here rather than duplicated: if the message format changes,
// there is one call to update instead of two that have to be kept in sync.
fn push_binary_op_mismatch(
    ctx: &mut CheckContext<'_, '_>,
    span: Span,
    operator: &str,
    left: TypeId,
    right: TypeId,
) {
    ctx.error(
        crate::diagnostic_messages::messages::binary_operand_type_mismatch(
            &ctx.arena, operator, left, right,
        ),
        span,
    );
}
