use oxc_ast::ast::{Expression, UnaryOperator};
use oxc_span::GetSpan;

use super::super::context::CheckContext;

// tsc applies two truthiness rules where an expression is tested for truthiness —
// the condition of if/while/do/for, a conditional expression's test, and the operand
// of `!` — and both are ported here so the checker agrees with tsc on exactly which
// conditions are flagged and never reports one tsc would not.
//
// The first rule is type-level (TS1345): an expression of type `void` cannot be
// tested for truthiness. `undefined` is deliberately not caught by it in tsc 6.0.3.
//
// The second rule is purely syntactic (getSyntacticTruthySemantics in checker.ts):
// literal-shaped expressions classify as always truthy or always falsy, and everything
// else — identifiers, member chains, calls — is "sometimes" and stays quiet. The
// classification reads the expression, not its type, which is why `if (0.0)` and
// `if (0x0)` are as quiet as `if (0)` while `if (2)` is always truthy, and why
// `if (true)` is left alone while `if (null)` is always falsy.
enum Truthiness {
    Always,
    Never,
    Sometimes,
}

pub(in crate::bridge) fn check_truthiness(
    expr: &Expression,
    ty: crate::arena::TypeId,
    ctx: &mut CheckContext<'_, '_>,
) {
    // tsc checks the type rule first and reports nothing else for a void expression.
    if matches!(ctx.arena.get(ty), crate::types::Type::Void) {
        ctx.error(
            crate::diagnostic_messages::messages::void_truthiness(),
            expr.span(),
        );
        return;
    }
    match syntactic_truthiness(expr) {
        Truthiness::Always => ctx.error(
            crate::diagnostic_messages::messages::always_truthy_expression(),
            expr.span(),
        ),
        Truthiness::Never => ctx.error(
            crate::diagnostic_messages::messages::always_falsy_expression(),
            expr.span(),
        ),
        Truthiness::Sometimes => {}
    }
}

fn syntactic_truthiness(expr: &Expression) -> Truthiness {
    match expr {
        // tsc leaves 0 and 1 alone as deliberately-constant conditions, by value: the
        // spellings 0.0 and 0x0 are as silent as 0. Any other numeric literal is one
        // or the other.
        Expression::NumericLiteral(n) => {
            if n.value == 0.0 || n.value == 1.0 {
                Truthiness::Sometimes
            } else {
                Truthiness::Always
            }
        }
        Expression::StringLiteral(s) => {
            if s.value.is_empty() {
                Truthiness::Never
            } else {
                Truthiness::Always
            }
        }
        // A template with no substitutions is the string literal it spells; a bad
        // escape with no cooked text is treated as empty, the way the checker types it.
        Expression::TemplateLiteral(template) if template.expressions.is_empty() => {
            let empty = template
                .quasis
                .first()
                .and_then(|quasi| quasi.value.cooked.as_ref())
                .is_none_or(|cooked| cooked.is_empty());
            if empty {
                Truthiness::Never
            } else {
                Truthiness::Always
            }
        }
        // `if (true)` and `if (false)` are tsc-quiet as well.
        Expression::BooleanLiteral(_) => Truthiness::Sometimes,
        Expression::NullLiteral(_) => Truthiness::Never,
        Expression::UnaryExpression(unary) if unary.operator == UnaryOperator::Void => {
            Truthiness::Never
        }
        // Everything that allocates is truthy, as in tsc's rule.
        Expression::ObjectExpression(_)
        | Expression::ArrayExpression(_)
        | Expression::ArrowFunctionExpression(_)
        | Expression::FunctionExpression(_)
        | Expression::ClassExpression(_)
        | Expression::BigIntLiteral(_)
        | Expression::RegExpLiteral(_)
        | Expression::JSXElement(_)
        | Expression::JSXFragment(_) => Truthiness::Always,
        // A ternary's truthiness is the two arms': both truthy is always truthy, both
        // falsy is always falsy, a mix is sometimes. tsc ORs the two classifications,
        // which is exactly this.
        Expression::ConditionalExpression(conditional) => {
            let consequent = syntactic_truthiness(&conditional.consequent);
            let alternate = syntactic_truthiness(&conditional.alternate);
            match (consequent, alternate) {
                (Truthiness::Always, Truthiness::Always) => Truthiness::Always,
                (Truthiness::Never, Truthiness::Never) => Truthiness::Never,
                _ => Truthiness::Sometimes,
            }
        }
        // Parens are the one wrapper tsc skips before classifying, so `if ((2))` is
        // always truthy like `if (2)`. oxc keeps parens as their own node.
        Expression::ParenthesizedExpression(inner) => syntactic_truthiness(&inner.expression),
        _ => Truthiness::Sometimes,
    }
}

// tsc's delete rules, on the operand as a whole (TS2703) and, for a named property,
// on its optionality (TS2790). The object side is inferred here rather than by the
// caller so its parts are visited once: the ordinary member lookup reports the
// missing property (TS2339) tsc reports for `delete o.zz`, and a property that does
// exist is checked for its optionality. A computed key that is not a literal, an
// `any` object and a private field are all left to tsc's silence — the optionality
// of the property cannot be read reliably there. Parentheses and an optional chain
// link change none of this, as in tsc.
pub(super) fn check_delete_operand(
    expr: &Expression,
    scoping: &oxc_semantic::Scoping,
    ctx: &mut CheckContext<'_, '_>,
) {
    // tsc reads through parentheses before applying the delete rules: `delete (o.p)`
    // is as valid as `delete o.p`, and `delete (1)` is not a property reference all
    // the same, with the diagnostic on the unwrapped operand. An optional chain
    // (`delete o?.p`) names its member the same way.
    let mut operand = expr;
    while let Expression::ParenthesizedExpression(inner) = operand {
        operand = &inner.expression;
    }
    match operand {
        Expression::StaticMemberExpression(member) => {
            check_named_member(
                &member.object,
                member.property.name.as_str(),
                member.span(),
                None,
                scoping,
                ctx,
            );
        }
        Expression::ComputedMemberExpression(member) => {
            // Only a string-literal key names a property reliably; `delete o[k]` is
            // tsc-quiet. The key itself is still inferred so its errors are reported.
            match &member.expression {
                Expression::StringLiteral(key) => {
                    infer_expression_of(&member.expression, scoping, ctx);
                    check_named_member(
                        &member.object,
                        key.value.as_str(),
                        member.span(),
                        None,
                        scoping,
                        ctx,
                    );
                }
                key => {
                    infer_expression_of(key, scoping, ctx);
                    infer_expression_of(&member.object, scoping, ctx);
                }
            }
        }
        // `delete o.#p` is a property reference as far as TS2703 goes; optionality is
        // not readable, so nothing further is said.
        Expression::PrivateFieldExpression(_) => {
            infer_expression_of(operand, scoping, ctx);
        }
        Expression::ChainExpression(chain) => match &chain.expression {
            oxc_ast::ast::ChainElement::StaticMemberExpression(member) => {
                check_named_member(
                    &member.object,
                    member.property.name.as_str(),
                    member.span(),
                    None,
                    scoping,
                    ctx,
                );
            }
            oxc_ast::ast::ChainElement::ComputedMemberExpression(member) => {
                match &member.expression {
                    Expression::StringLiteral(key) => {
                        infer_expression_of(&member.expression, scoping, ctx);
                        check_named_member(
                            &member.object,
                            key.value.as_str(),
                            member.span(),
                            None,
                            scoping,
                            ctx,
                        );
                    }
                    key => {
                        infer_expression_of(key, scoping, ctx);
                        infer_expression_of(&member.object, scoping, ctx);
                    }
                }
            }
            oxc_ast::ast::ChainElement::PrivateFieldExpression(_) => {
                infer_expression_of(operand, scoping, ctx);
            }
            // `delete o?.zz!` or a dangling call in a chain: the member or call keeps
            // its own checking through the ordinary read below; tsc adds no delete
            // rule of its own for these.
            _ => {
                infer_expression_of(expr, scoping, ctx);
            }
        },
        // The operand's own errors (an undeclared name, say) are reported on the way
        // to the grammar diagnostic, as tsc reports both.
        other => {
            infer_expression_of(other, scoping, ctx);
            ctx.error(
                crate::diagnostic_messages::messages::delete_operand_not_property(),
                other.span(),
            );
        }
    }
}

// One named property on `delete`, through a direct or an optional-chain member: the
// ordinary member lookup runs for its own TS2339, and a property that exists is
// checked for its optionality (TS2790), which is tsc's only further word.
fn check_named_member(
    object_expr: &Expression,
    property_name: &str,
    member_span: oxc_span::Span,
    key_expr: Option<&Expression>,
    scoping: &oxc_semantic::Scoping,
    ctx: &mut CheckContext<'_, '_>,
) {
    if let Some(key) = key_expr {
        infer_expression_of(key, scoping, ctx);
    }
    let object_type = infer_expression_of(object_expr, scoping, ctx);
    let property_type = super::members::infer_member_access_type_with_optional(
        object_type,
        property_name,
        false,
        member_span,
        ctx,
    );
    if matches!(ctx.arena.get(property_type), crate::types::Type::Error) {
        // A missing property was already reported by the member lookup; there is no
        // property whose optionality could be checked, as in tsc.
        return;
    }
    if let crate::types::Type::Object(object) = ctx.arena.get(object_type)
        && let Some(property) = object.properties.iter().find(|p| *p.name == *property_name)
        && !property.optional
    {
        ctx.error(
            crate::diagnostic_messages::messages::delete_operand_required(),
            member_span,
        );
    }
}

// A thin alias over infer_expression_type kept for readability of the delete walk,
// which infers several parts itself instead of relying on the dispatch arm.
fn infer_expression_of(
    expr: &Expression,
    scoping: &oxc_semantic::Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> crate::arena::TypeId {
    super::infer_expression_type(expr, scoping, ctx)
}

// TS2695 decides on the shape of the discarded left side: a call, a construction, an
// assignment, a `++`/`--`, an await, a yield, a tagged template or a delete has an
// effect; a read — a name, a member chain, a literal — does not. A conditional or a
// logical operator may run an effectful arm, so it is treated as effectful rather
// than risk a false positive on `(cond, f(), x)`. Parens unwrap to their inside.
pub(super) fn has_side_effects(expr: &Expression) -> bool {
    match expr {
        Expression::CallExpression(_)
        | Expression::NewExpression(_)
        | Expression::AssignmentExpression(_)
        | Expression::UpdateExpression(_)
        | Expression::AwaitExpression(_)
        | Expression::LogicalExpression(_)
        | Expression::ConditionalExpression(_)
        | Expression::ChainExpression(_) => true,
        Expression::UnaryExpression(unary) => {
            if unary.operator == UnaryOperator::Delete {
                true
            } else {
                has_side_effects(&unary.argument)
            }
        }
        Expression::ParenthesizedExpression(inner) => has_side_effects(&inner.expression),
        _ => false,
    }
}

// tsc's `isIndirectCall`: `(0, o.f)()` and `(0, eval)()` call a function without its `this`,
// and the `0` on the left is how that is written, so it is not reported as unused. The comma
// expression must be exactly `0, <access>` (or `0, eval`), parenthesized, and the callee.
pub(super) fn note_indirect_call(callee: &Expression, ctx: &mut CheckContext<'_, '_>) {
    let Expression::ParenthesizedExpression(paren) = callee else {
        return;
    };
    let Expression::SequenceExpression(sequence) = &paren.expression else {
        return;
    };
    let [Expression::NumericLiteral(zero), right] = sequence.expressions.as_slice() else {
        return;
    };
    if zero.raw.as_ref().is_none_or(|raw| raw.as_str() != "0") {
        return;
    }
    let is_access = matches!(
        right,
        Expression::StaticMemberExpression(_)
            | Expression::ComputedMemberExpression(_)
            | Expression::PrivateFieldExpression(_)
    );
    let is_eval = matches!(right, Expression::Identifier(name) if name.name == "eval");
    if is_access || is_eval {
        ctx.indirect_call_sequences.insert(sequence.span.start);
    }
}
