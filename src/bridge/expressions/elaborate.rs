use oxc_ast::ast::{Expression, ObjectPropertyKind};
use oxc_span::{GetSpan, Span};

use crate::arena::TypeId;
use crate::diagnostic_codes::DiagnosticCode;
use crate::diagnostic_messages::{DiagnosticMessage, messages};
use crate::types::Type;

use super::super::context::CheckContext;

// tsc does not report a bad object literal as one error on the whole literal when it
// can say which part is wrong. `const p: Point = { x: 1, y: "s" }` is an error on `y`
// that reads "Type 'string' is not assignable to type 'number'.", one per bad
// property, and a literal that is only missing a property is still reported on the
// whole (TS2741). An arrow function with an expression body and nothing annotated is
// the same: the error lands on the body, as the return type that does not fit.
//
// Reports those errors and says whether it reported any. When it returns false the
// caller reports `whole` on the whole expression, so the message tsc would give for
// the expression as a whole is never lost.
//
// Called only after the ordinary assignability check has already failed.
pub(crate) fn elaborate_mismatch(
    expr: &Expression,
    actual: TypeId,
    expected: TypeId,
    ctx: &mut CheckContext<'_, '_>,
) -> bool {
    match expr {
        Expression::ParenthesizedExpression(inner) => {
            elaborate_mismatch(&inner.expression, actual, expected, ctx)
        }
        // One error per branch that does not fit, each on the branch.
        Expression::ConditionalExpression(conditional) => {
            let Some(&(consequent_type, alternate_type)) =
                ctx.conditional_arms.get(&conditional.span.start)
            else {
                return false;
            };
            let mut reported = false;
            for (branch, branch_type) in [
                (&conditional.consequent, consequent_type),
                (&conditional.alternate, alternate_type),
            ] {
                if ctx.semantic().is_assignable(branch_type, expected) {
                    continue;
                }
                if !elaborate_mismatch(branch, branch_type, expected, ctx) {
                    let message = messages::assignability(
                        &ctx.arena,
                        branch_type,
                        expected,
                        DiagnosticCode::DeclaredTypeMismatch,
                    );
                    ctx.error(message, branch.span());
                }
                reported = true;
            }
            reported
        }
        Expression::ObjectExpression(object) => {
            let (Type::Object(actual_object), Type::Object(expected_object)) = (
                ctx.arena.get(actual).clone(),
                ctx.arena.get(expected).clone(),
            ) else {
                return false;
            };

            let mut reported = false;
            for property in &object.properties {
                let ObjectPropertyKind::ObjectProperty(property) = property else {
                    continue;
                };
                if property.computed || property.method {
                    continue;
                }
                let Some(name) = property.key.static_name() else {
                    continue;
                };
                let (Some(have), Some(wanted)) = (
                    actual_object.properties.iter().find(|p| *p.name == *name),
                    expected_object.properties.iter().find(|p| *p.name == *name),
                ) else {
                    continue;
                };
                if ctx.semantic().is_assignable(have.type_id, wanted.type_id) {
                    continue;
                }
                if elaborate_mismatch(&property.value, have.type_id, wanted.type_id, ctx) {
                    reported = true;
                    continue;
                }
                let message = messages::assignability(
                    &ctx.arena,
                    have.type_id,
                    wanted.type_id,
                    DiagnosticCode::DeclaredTypeMismatch,
                );
                ctx.error(message, property.key.span());
                reported = true;
            }
            reported
        }
        Expression::ArrayExpression(array) => {
            // `const xs: number[] = [1, "a"]` is an error on `"a"` alone. Only elements
            // whose type is known from the text (a literal) are taken apart; for any
            // other element the whole expression is reported, on the same line.
            //
            // Against a tuple the same is done position by position, as tsc does: an
            // element that fits no position is left to the whole literal's error (a literal
            // of the wrong length is reported as one mismatch, not element by element). A
            // spread makes positions unknown, so a tuple target is then not taken apart.
            let expected_type = ctx.arena.get(expected).clone();
            if !matches!(expected_type, Type::Array(_) | Type::Tuple(_)) {
                return false;
            }
            if matches!(expected_type, Type::Tuple(_))
                && array
                    .elements
                    .iter()
                    .any(|e| matches!(e, oxc_ast::ast::ArrayExpressionElement::SpreadElement(_)))
            {
                return false;
            }
            let wanted_at = |position: usize| -> Option<TypeId> {
                match &expected_type {
                    Type::Array(element_type) => Some(*element_type),
                    Type::Tuple(elements) => {
                        let rest = elements.last().filter(|e| e.rest);
                        let fixed = elements.len() - usize::from(rest.is_some());
                        // A position that lands on the rest element is not taken apart: tsc
                        // reports the whole literal there, with the range of positions in the
                        // reason beneath it.
                        if position < fixed {
                            Some(elements[position].type_id)
                        } else {
                            None
                        }
                    }
                    _ => None,
                }
            };
            let mut reported = false;
            for (position, element) in array.elements.iter().enumerate() {
                let Some(element_expr) = element.as_expression() else {
                    continue;
                };
                let Some(element_type) = wanted_at(position) else {
                    continue;
                };
                let have = match element_expr {
                    Expression::StringLiteral(text) => {
                        ctx.arena.alloc(Type::StringLiteral(text.value.to_string()))
                    }
                    Expression::NumericLiteral(number) => {
                        ctx.arena.alloc(Type::NumberLiteral(number.value))
                    }
                    Expression::BooleanLiteral(boolean) => {
                        ctx.arena.alloc(Type::BooleanLiteral(boolean.value))
                    }
                    Expression::NullLiteral(_) => ctx.arena.null(),
                    _ => continue,
                };
                if ctx.semantic().is_assignable(have, element_type) {
                    continue;
                }
                let message = messages::assignability(
                    &ctx.arena,
                    have,
                    element_type,
                    DiagnosticCode::DeclaredTypeMismatch,
                );
                ctx.error(message, element_expr.span());
                reported = true;
            }
            reported
        }
        Expression::ArrowFunctionExpression(arrow) => {
            // tsc leaves an arrow alone when any parameter is annotated, when it has
            // type parameters or a return annotation, and when the body is a block.
            if arrow.type_parameters.is_some()
                || arrow.return_type.is_some()
                || arrow
                    .params
                    .items
                    .iter()
                    .any(|p| p.type_annotation.is_some())
                || arrow.params.rest.is_some()
            {
                return false;
            }
            let Some(body) = arrow.body.as_expression() else {
                return false;
            };
            let (Type::Function(actual_fn), Type::Function(expected_fn)) = (
                ctx.arena.get(actual).clone(),
                ctx.arena.get(expected).clone(),
            ) else {
                return false;
            };
            if ctx
                .semantic()
                .is_assignable(actual_fn.return_type, expected_fn.return_type)
            {
                return false;
            }
            if elaborate_mismatch(body, actual_fn.return_type, expected_fn.return_type, ctx) {
                return true;
            }
            let message = messages::assignability(
                &ctx.arena,
                actual_fn.return_type,
                expected_fn.return_type,
                DiagnosticCode::DeclaredTypeMismatch,
            );
            ctx.error(message, body.span());
            true
        }
        _ => false,
    }
}

// The common ending of every site that checks an expression against a type: elaborate
// into the expression when tsc would, and otherwise report `whole` on `span`.
pub(crate) fn report_mismatch(
    expr: Option<&Expression>,
    actual: TypeId,
    expected: TypeId,
    whole: DiagnosticMessage,
    span: Span,
    ctx: &mut CheckContext<'_, '_>,
) {
    if let Some(expr) = expr
        && elaborate_mismatch(expr, actual, expected, ctx)
    {
        return;
    }
    ctx.error(whole, span);
}
