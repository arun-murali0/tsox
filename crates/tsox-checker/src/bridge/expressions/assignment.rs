use oxc_ast::ast::{AssignmentExpression, AssignmentOperator, AssignmentTarget, BinaryOperator};
use oxc_semantic::{Scoping, SymbolId};
use oxc_span::GetSpan;

use crate::arena::TypeId;
use crate::types::Type;

use super::super::context::CheckContext;
use super::super::narrow::{path_of_member, resolve_symbol_id};
use super::binary::infer_binary_expression_type;
use super::core::resolve_identifier_type;
use super::infer_expression_type;
use super::members::{infer_computed_member_access_type, infer_member_access_type};

// `x = y` and `x op= y`. The right-hand side is always inferred (and so
// always gets whatever errors it has on its own), regardless of whether the
// target itself can be resolved.
pub(super) fn infer_assignment_expression_type(
    assign: &AssignmentExpression,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> TypeId {
    // TS2588 and TS2632: a `const` or an import binding cannot be written to, whatever the
    // right-hand side holds. The right side keeps its own errors, and the write is not also
    // checked against the binding's type, which tsc does not do either.
    if let AssignmentTarget::AssignmentTargetIdentifier(ident) = &assign.left {
        if let Some(message) = immutable_binding_message(ident, scoping, ctx) {
            let right_type = infer_expression_type(&assign.right, scoping, ctx);
            ctx.error(message, ident.span);
            return right_type;
        }
    }
    // The target is the contextual type of the right-hand side. It is read first, which
    // reports nothing the right side's own inference would not (the target is only looked
    // up here), and an unresolvable target simply gives no context.
    let target_type = resolve_assignment_target_type(&assign.left, scoping, ctx);
    if matches!(assign.operator, AssignmentOperator::Assign) {
        ctx.contextual = target_type;
    }
    let right_type = infer_expression_type(&assign.right, scoping, ctx);

    let Some(target_type) = target_type else {
        // A destructuring target (`[a, b] = arr`, `({x} = obj)`), or one
        // wrapped in `as`/`!`/a type assertion, not modeled. Skips the
        // assignability check rather than guessing; the right-hand side's own
        // errors above still stand.
        return right_type;
    };

    match assign.operator {
        AssignmentOperator::Assign => {
            // A plain assignment to a variable is checked against the type it was
            // declared with, not the narrowed type it happens to have at this
            // point: `if (x !== null) { x = null; }` is valid for a
            // `string | null`. Compound assignment below keeps the narrowed type,
            // since it reads the current value.
            let declared = declared_identifier_target(&assign.left, scoping, ctx);
            let check_against = declared.map_or(target_type, |(_, declared_type)| declared_type);
            if !ctx.is_assignable_instantiating(right_type, check_against) {
                let whole = crate::diagnostic_messages::messages::declared_type_mismatch(
                    &ctx.arena,
                    right_type,
                    check_against,
                );
                super::elaborate::report_mismatch(
                    Some(&assign.right),
                    right_type,
                    check_against,
                    whole,
                    assign.span(),
                    ctx,
                );
            }
            if let Some((symbol_id, declared_type)) = declared {
                narrow_on_assignment(symbol_id, declared_type, right_type, ctx);
            }
            narrow_member_on_assignment(&assign.left, target_type, right_type, scoping, ctx);
            right_type
        }
        // `x op= y` is `x = x op y`: whatever the operator's own rule is,
        // applied with the target's current type on the left. That reuses
        // infer_binary_expression_type's own mismatch reporting (e.g. `x -=
        // "a"` reports the same way `x - "a"` would). Not checked here, and a
        // narrower gap than tsc: whether the *result* is itself assignable
        // back to the target's type (`x: "a" | "b"; x += "c";` is invalid in
        // tsc since the result widens to plain string), left for later.
        operator => match compound_assignment_operator(operator) {
            Some(binary_operator) => infer_binary_expression_type(
                binary_operator,
                target_type,
                right_type,
                assign.span(),
                ctx,
            ),
            // Logical assignment (&&=, ||=, ??=): short-circuiting, so not a
            // plain binary op the way the arithmetic ones are. Not modeled.
            None => right_type,
        },
    }
}

// `x++` and `--x`. The operand is read for its own errors; a `const` or an import cannot be
// written to; the result is `number`. This checker has no `bigint` type, so a bigint operand
// reads as `number` too.
pub(super) fn infer_update_expression_type(
    update: &oxc_ast::ast::UpdateExpression,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> TypeId {
    use oxc_ast::ast::SimpleAssignmentTarget;
    if let SimpleAssignmentTarget::AssignmentTargetIdentifier(ident) = &update.argument {
        if let Some(message) = immutable_binding_message(ident, scoping, ctx) {
            ctx.error(message, ident.span);
            return ctx.arena.number();
        }
        // The operand is read for its own errors (an undeclared name, say).
        resolve_identifier_type(ident, scoping, ctx);
    }
    // `p.n++` writes the property, so a readonly one is an error as in a plain assignment.
    if let SimpleAssignmentTarget::StaticMemberExpression(member) = &update.argument {
        let object_type = infer_expression_type(&member.object, scoping, ctx);
        super::super::accessibility::check_readonly_write(
            ctx,
            object_type,
            matches!(member.object, oxc_ast::ast::Expression::ThisExpression(_)),
            &member.property.name,
            member.property.span,
        );
    }
    ctx.arena.number()
}

fn immutable_binding_message(
    ident: &oxc_ast::ast::IdentifierReference,
    scoping: &Scoping,
    ctx: &CheckContext<'_, '_>,
) -> Option<crate::diagnostic_messages::DiagnosticMessage> {
    let Some(symbol_id) = resolve_symbol_id(ident, scoping) else {
        // No binding in this file: a global another file declared `const` is still read-only.
        return ctx
            .namespace
            .is_const_global(&ident.name)
            .then(|| crate::diagnostic_messages::messages::assignment_to_constant(&ident.name));
    };
    let flags = scoping.symbol_flags(symbol_id);
    if flags.is_const_variable() {
        Some(crate::diagnostic_messages::messages::assignment_to_constant(&ident.name))
    } else if flags.is_import() {
        Some(crate::diagnostic_messages::messages::assignment_to_import(
            &ident.name,
        ))
    } else {
        None
    }
}

// The symbol and declared type behind a plain identifier target, read from the
// symbol table rather than the narrowing overlay. None for any other target, or an
// identifier with no registered type, in which case the caller falls back to the
// type resolve_assignment_target_type produced.
fn declared_identifier_target(
    target: &AssignmentTarget,
    scoping: &Scoping,
    ctx: &CheckContext<'_, '_>,
) -> Option<(SymbolId, TypeId)> {
    let AssignmentTarget::AssignmentTargetIdentifier(ident) = target else {
        return None;
    };
    let symbol_id = resolve_symbol_id(ident, scoping)?;
    let declared = ctx.symbols.get(symbol_id)?;
    Some((symbol_id, declared))
}

// After `x = value`, x reads as the members of its declared union that the
// assigned type can be assigned to (tsc's assignment narrowing), or as the declared
// type when that leaves nothing or the declared type is not a union. Either way
// this replaces any overlay entry a guard left behind, which would otherwise be
// stale the moment x holds a new value.
pub fn narrow_on_assignment(
    symbol_id: SymbolId,
    declared: TypeId,
    assigned: TypeId,
    ctx: &mut CheckContext<'_, '_>,
) {
    // Whatever was known about `x.a`, `x.a.b` described the old value of x.
    ctx.narrow.remove_paths_of(symbol_id);
    let members = match ctx.arena.get(declared) {
        Type::Union(members) => members.clone(),
        _ => {
            ctx.narrow.insert(symbol_id, declared);
            return;
        }
    };
    let mut kept = Vec::with_capacity(members.len());
    for member in members {
        if ctx.semantic().is_assignable(assigned, member) {
            kept.push(member);
        }
    }
    let narrowed = if kept.is_empty() {
        declared
    } else {
        ctx.arena.alloc_union(kept)
    };
    ctx.narrow.insert(symbol_id, narrowed);
}

// `user.address = value`: what was known about `user.address` (and anything under it) ends,
// and a union-typed property then reads as the members the assigned type fits, the same
// rule narrow_on_assignment applies to a variable. `declared` is the property's declared
// type, which resolve_assignment_target_type reads without consulting any path narrowing.
fn narrow_member_on_assignment(
    target: &AssignmentTarget,
    declared: TypeId,
    assigned: TypeId,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) {
    let AssignmentTarget::StaticMemberExpression(member) = target else {
        return;
    };
    let Some(reference) = path_of_member(member, scoping) else {
        return;
    };
    ctx.narrow
        .remove_path_prefix(reference.symbol_id, &reference.path);

    let Type::Union(members) = ctx.arena.get(declared) else {
        return;
    };
    let members = members.clone();
    let mut kept = Vec::with_capacity(members.len());
    for member_type in members {
        if ctx.semantic().is_assignable(assigned, member_type) {
            kept.push(member_type);
        }
    }
    if kept.is_empty() {
        return;
    }
    let narrowed = ctx.arena.alloc_union(kept);
    if narrowed != declared {
        ctx.narrow
            .insert_path(reference.symbol_id, reference.path, narrowed);
    }
}

// The type a write lands on. None for a target with no type to compare against
// (destructuring, a type assertion), which skips the assignability check instead of
// guessing; a wrong error is worse than a missing one.
fn resolve_assignment_target_type(
    target: &AssignmentTarget,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> Option<TypeId> {
    match target {
        AssignmentTarget::AssignmentTargetIdentifier(ident) => {
            Some(resolve_identifier_type(ident, scoping, ctx))
        }
        AssignmentTarget::StaticMemberExpression(member) => {
            let object_type = infer_expression_type(&member.object, scoping, ctx);
            // tsc reports the readonly write and nothing about the value's type.
            if super::super::accessibility::check_readonly_write(
                ctx,
                object_type,
                matches!(member.object, oxc_ast::ast::Expression::ThisExpression(_)),
                &member.property.name,
                member.property.span,
            ) {
                return None;
            }
            Some(infer_member_access_type(
                object_type,
                &member.property.name,
                member.span(),
                ctx,
            ))
        }
        AssignmentTarget::ComputedMemberExpression(member) => {
            let object_type = infer_expression_type(&member.object, scoping, ctx);
            if ctx.arena.is_readonly_array(object_type) {
                ctx.error(
                    crate::diagnostic_messages::messages::readonly_index_write(
                        &ctx.arena,
                        object_type,
                    ),
                    member.span(),
                );
                return None;
            }
            Some(infer_computed_member_access_type(
                object_type,
                &member.expression,
                member.optional,
                member.span(),
                scoping,
                ctx,
            ))
        }
        _ => None,
    }
}

// The arithmetic compound forms, mapped to the binary operation they stand for, so `x += y`
// is checked with the operand rules of `x + y`. The logical forms short-circuit and the
// rest are not modelled; both return None and the caller falls back to the right side.
fn compound_assignment_operator(operator: AssignmentOperator) -> Option<BinaryOperator> {
    Some(match operator {
        AssignmentOperator::Addition => BinaryOperator::Addition,
        AssignmentOperator::Subtraction => BinaryOperator::Subtraction,
        AssignmentOperator::Multiplication => BinaryOperator::Multiplication,
        AssignmentOperator::Division => BinaryOperator::Division,
        AssignmentOperator::Remainder => BinaryOperator::Remainder,
        AssignmentOperator::Exponential => BinaryOperator::Exponential,
        _ => return None,
    })
}
