use oxc_ast::ast::Expression;
use oxc_semantic::Scoping;
use oxc_span::GetSpan;

use crate::arena::TypeId;
use crate::types::Type;

use super::context::CheckContext;

mod assignment;
mod binary;
mod calls;
mod core;
mod elaborate;
mod excess;
mod functions;
mod logical;
mod members;
mod objects;

pub(super) use assignment::narrow_on_assignment;
pub(super) use elaborate::report_mismatch;
pub(super) use excess::check_excess_properties;
pub(super) use members::infer_member_access_type;

use assignment::infer_assignment_expression_type;
use binary::infer_binary_expression_type;
use calls::{infer_call_expression_type, infer_new_expression_type};
pub(super) use core::resolve_identifier_type;
use functions::{infer_arrow_function_type, infer_function_expression_type};
use logical::{
    infer_as_expression_type, infer_conditional_expression_type, infer_logical_expression_type,
    infer_non_null_expression_type,
};
use members::{
    infer_chain_element_type, infer_computed_member_access_type,
    infer_member_access_type_with_optional,
};
use objects::{infer_array_expression_type, infer_object_expression_type};

// The single dispatch point for every expression kind this checker understands.
// Each arm delegates to a feature module (binary.rs, calls.rs, and so on); this
// function's own job is only to route, not to hold checking logic itself. An
// expression kind with no arm here falls through to the catch-all at the bottom,
// which reports it honestly as unsupported rather than guessing a type for it.
pub fn infer_expression_type(
    expr: &Expression,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> TypeId {
    let expected = ctx.contextual.take();
    if crate::stack_guard::exhausted() {
        return ctx.nesting_too_deep(expr.span());
    }
    match expr {
        Expression::NumericLiteral(n) => ctx.arena.alloc(Type::NumberLiteral(n.value)),
        Expression::StringLiteral(s) => ctx.arena.alloc(Type::StringLiteral(s.value.to_string())),
        Expression::BooleanLiteral(b) => ctx.arena.alloc(Type::BooleanLiteral(b.value)),
        Expression::NullLiteral(_) => ctx.arena.null(),

        Expression::Identifier(ident) => resolve_identifier_type(ident, scoping, ctx),

        // Outside any class method body, current_class_instance is None. Inside a
        // function known to have no `this` at all that is an implicit any, and is
        // reported. Anywhere else it may be legitimate (an object literal method,
        // a callback whose signature supplies `this`), so it is left as an Error
        // sentinel and whatever this expression's result feeds into is what gets
        // flagged, if anything.
        Expression::ThisExpression(this) => match ctx.current_class_instance {
            Some(instance) => instance,
            None => {
                if ctx.implicit_this {
                    ctx.error(
                        crate::diagnostic_messages::messages::this_implicitly_any(),
                        this.span,
                    );
                }
                ctx.arena.error()
            }
        },

        Expression::BinaryExpression(bin) => {
            let left = infer_expression_type(&bin.left, scoping, ctx);
            let right = infer_expression_type(&bin.right, scoping, ctx);
            let (left, right) = binary::strip_possibly_nullish(
                bin.operator,
                [(&bin.left, left), (&bin.right, right)],
                ctx,
            );
            infer_binary_expression_type(bin.operator, left, right, bin.span(), ctx)
        }

        Expression::CallExpression(call) => infer_call_expression_type(call, scoping, ctx),

        Expression::NewExpression(new_expr) => infer_new_expression_type(new_expr, scoping, ctx),

        // `user.address` narrowed by an earlier `user.address !== null` reads as that
        // narrowed type; every other access is typed from its object as before.
        Expression::StaticMemberExpression(member) => {
            match super::narrow::narrowed_member_type(member, scoping, ctx) {
                Some(narrowed) => narrowed,
                None => {
                    let object_type = infer_expression_type(&member.object, scoping, ctx);
                    let (object_type, short_circuits) = members::strip_nullish_object(
                        &member.object,
                        object_type,
                        member.optional,
                        ctx,
                    );
                    let property_type = infer_member_access_type_with_optional(
                        object_type,
                        &member.property.name,
                        member.optional,
                        member.span(),
                        ctx,
                    );
                    members::with_chain_undefined(property_type, short_circuits, ctx)
                }
            }
        }

        Expression::ComputedMemberExpression(member) => {
            let object_type = infer_expression_type(&member.object, scoping, ctx);
            let (object_type, short_circuits) =
                members::strip_nullish_object(&member.object, object_type, member.optional, ctx);
            let element_type = infer_computed_member_access_type(
                object_type,
                &member.expression,
                member.optional,
                member.span(),
                scoping,
                ctx,
            );
            members::with_chain_undefined(element_type, short_circuits, ctx)
        }

        Expression::ChainExpression(chain) => {
            infer_chain_element_type(&chain.expression, scoping, ctx)
        }

        Expression::ObjectExpression(object) => {
            infer_object_expression_type(object, expected, scoping, ctx)
        }

        Expression::ArrayExpression(array) => {
            infer_array_expression_type(array, expected, scoping, ctx)
        }

        Expression::ArrowFunctionExpression(arrow) => {
            infer_arrow_function_type(arrow, scoping, ctx)
        }

        Expression::FunctionExpression(func) => infer_function_expression_type(func, scoping, ctx),

        Expression::LogicalExpression(logical) => {
            infer_logical_expression_type(logical, scoping, ctx)
        }

        Expression::ConditionalExpression(conditional) => {
            infer_conditional_expression_type(conditional, scoping, ctx)
        }

        Expression::TSAsExpression(as_expr) => infer_as_expression_type(as_expr, scoping, ctx),

        Expression::TSNonNullExpression(non_null) => {
            infer_non_null_expression_type(non_null, scoping, ctx)
        }

        Expression::AssignmentExpression(assign) => {
            infer_assignment_expression_type(assign, scoping, ctx)
        }

        Expression::UnaryExpression(unary)
            if unary.operator == oxc_ast::ast::UnaryOperator::Typeof =>
        {
            // The operand is still checked for its own errors, but typeof's
            // result is always the literal runtime string "string", "number", and
            // so on, so the operand's own inferred type is discarded here rather
            // than reused.
            infer_expression_type(&unary.argument, scoping, ctx);
            ctx.arena.string()
        }

        // Same shape as typeof just above: check the operand for its own errors,
        // but !'s result is always boolean regardless of what the operand is, so
        // its inferred type is discarded rather than reused.
        Expression::UnaryExpression(unary)
            if unary.operator == oxc_ast::ast::UnaryOperator::LogicalNot =>
        {
            infer_expression_type(&unary.argument, scoping, ctx);
            ctx.arena.boolean()
        }

        // Problem: `-x`, `+x`, `~x`, `void x` and `delete x` reached the catch-all, so
        // anything built on them (`-1`, `const n = -total`) was an error type and
        // lost its checking.
        // Picked: the operand is still checked for its own errors. `-1` keeps the
        // literal -1 so `const x = -1` reads as a literal like `const x = 1` does;
        // every other numeric operator gives number, void gives undefined, delete
        // gives boolean.
        // Cost: the operand's own type is not checked to be number-like; tsc reports
        // that for `-"a"` only under stricter rules, so nothing is lost today.
        Expression::UnaryExpression(unary) => {
            use oxc_ast::ast::UnaryOperator;
            let operand = infer_expression_type(&unary.argument, scoping, ctx);
            match unary.operator {
                UnaryOperator::UnaryNegation => match &unary.argument {
                    Expression::NumericLiteral(n) => ctx.arena.alloc(Type::NumberLiteral(-n.value)),
                    _ => ctx.arena.number(),
                },
                UnaryOperator::UnaryPlus | UnaryOperator::BitwiseNot => ctx.arena.number(),
                UnaryOperator::Void => ctx.arena.undefined(),
                UnaryOperator::Delete => ctx.arena.boolean(),
                _ => {
                    let _ = operand;
                    ctx.arena.error()
                }
            }
        }

        // Problem: a template literal hit the catch-all, and an interpolated
        // expression inside it was never checked at all.
        // Picked: every interpolation is checked for its own errors. A template with
        // no interpolation is the string literal it spells; one with interpolation is
        // string, the way tsc types it.
        // Cost: a template with no interpolation and a bad escape (no cooked text)
        // falls back to string.
        Expression::TemplateLiteral(template) => {
            for inner in &template.expressions {
                infer_expression_type(inner, scoping, ctx);
            }
            if template.expressions.is_empty()
                && let Some(only) = template.quasis.first()
                && let Some(cooked) = &only.value.cooked
            {
                return ctx.arena.alloc(Type::StringLiteral(cooked.to_string()));
            }
            ctx.arena.string()
        }

        // oxc keeps parens as their own node instead of discarding them, so without
        // this a parenthesized condition (`!(typeof x === "number")`) would hit the
        // catch-all below purely because of the parens, on top of narrow_condition
        // needing the same unwrap for narrowing to see through it.
        Expression::ParenthesizedExpression(inner) => {
            infer_expression_type(&inner.expression, scoping, ctx)
        }

        // `await x` is the T of a Promise<T>; anything that is not a Promise is its own value.
        Expression::AwaitExpression(await_expr) => {
            let operand = infer_expression_type(&await_expr.argument, scoping, ctx);
            ctx.arena.promise_value_type(operand).unwrap_or(operand)
        }

        // `import("x")` is a Promise of the module object `typeof import("x")`, which the
        // import pass bound under that name when the specifier is a plain string and the
        // module's exports are known in full. Anything else is a Promise of `any`.
        Expression::ImportExpression(import) => {
            if let Some(specifier) = import_specifier_text(&import.source) {
                let name = format!("typeof import({specifier:?})");
                if let crate::namespace::Resolution::Resolved(module) =
                    ctx.namespace.resolve(&name, &mut ctx.arena)
                {
                    return ctx.arena.alloc_promise(module);
                }
            }
            let any = ctx.arena.any();
            ctx.arena.alloc_promise(any)
        }

        _ => {
            ctx.warning(
                crate::diagnostic_messages::messages::unimplemented_expression_kind(),
                expr.span(),
            );
            ctx.arena.error()
        }
    }
}

// The text of an `import("x")` specifier when it is a plain string: a string literal or a
// template with no substitutions.
pub(super) fn import_specifier_text(source: &Expression<'_>) -> Option<String> {
    match source {
        Expression::StringLiteral(literal) => Some(literal.value.to_string()),
        Expression::TemplateLiteral(template) if template.expressions.is_empty() => template
            .quasis
            .first()
            .and_then(|quasi| quasi.value.cooked.as_ref())
            .map(|cooked| cooked.to_string()),
        _ => None,
    }
}
