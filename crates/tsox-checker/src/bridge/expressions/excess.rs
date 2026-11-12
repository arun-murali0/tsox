use oxc_ast::ast::{Expression, ObjectPropertyKind, PropertyKey};

use crate::arena::TypeId;
use crate::types::Type;

use super::super::context::CheckContext;

// TypeScript's excess property check: a *fresh* object literal, one written
// directly where a target type is expected, may not carry a property the target
// does not declare. The same object assigned through a variable first is no
// longer fresh, which is why this inspects the expression itself rather than
// living in structural subtyping (where `{ x, y }` is, correctly, a subtype of
// `{ x }`).
//
// Called only after the ordinary assignability check has already passed, so a
// literal that is wrong in some other way reports that one error, not two.
// Applies at the three places a literal meets an expected type today: a
// variable's declared type, a call argument, and a return statement. Recurses
// into nested literals whose target property is itself an object type. A target
// that is not a plain object type (a union, an unresolved generic parameter) is
// skipped, which is always safe, just less precise.
pub fn check_excess_properties(expr: &Expression, target: TypeId, ctx: &mut CheckContext<'_, '_>) {
    let Expression::ObjectExpression(object) = expr else {
        return;
    };
    // Record<K, V> and Promise<T> have an empty implementation body (see
    // type_annotation::resolve_builtin_generic). A Record literal must satisfy both
    // parts of its contract: every statically named key must belong to K, and its value
    // must fit V. Do not treat the second argument as an unrestricted index signature.
    if let (Some(key_domain), Some(value_type)) = (
        ctx.arena.record_key_type(target),
        ctx.arena.record_value_type(target),
    ) {
        for property in &object.properties {
            let ObjectPropertyKind::ObjectProperty(property) = property else {
                continue;
            };
            let (name, key_type, key_span) = match &property.key {
                PropertyKey::StaticIdentifier(key) => (
                    key.name.to_string(),
                    ctx.arena.alloc(Type::StringLiteral(key.name.to_string())),
                    key.span,
                ),
                PropertyKey::StringLiteral(key) => (
                    key.value.to_string(),
                    ctx.arena.alloc(Type::StringLiteral(key.value.to_string())),
                    key.span,
                ),
                PropertyKey::NumericLiteral(key) => (
                    key.value.to_string(),
                    ctx.arena.alloc(Type::NumberLiteral(key.value)),
                    key.span,
                ),
                // Computed and spread keys do not have a statically known member name.
                _ => continue,
            };
            if !crate::subtyping::is_subtype(&ctx.arena, key_type, key_domain) {
                ctx.error(
                    crate::diagnostic_messages::messages::excess_property(
                        &ctx.arena, &name, target,
                    ),
                    key_span,
                );
                continue;
            }
            check_excess_properties(&property.value, value_type, ctx);
        }
        return;
    }
    if ctx
        .arena
        .app_parts(target)
        .is_some_and(|(name, _)| name == "Promise")
    {
        return;
    }
    let Type::Object(target_object) = ctx.arena.get(target).clone() else {
        return;
    };

    for property in &object.properties {
        // Spreads and computed keys carry no statically known name to check.
        let ObjectPropertyKind::ObjectProperty(property) = property else {
            continue;
        };
        let PropertyKey::StaticIdentifier(key) = &property.key else {
            continue;
        };

        match target_object
            .properties
            .iter()
            .find(|p| *p.name == *key.name.as_str())
        {
            // A name the target does not list is the index signature's business.
            None if target_object.string_index.is_some()
                || (target_object.number_index.is_some()
                    && key.name.as_str().chars().all(|c| c.is_ascii_digit())) =>
            {
                let value = target_object
                    .string_index
                    .as_ref()
                    .or(target_object.number_index.as_ref())
                    .map(|index| index.value);
                if let Some(value) = value {
                    check_excess_properties(&property.value, value, ctx);
                }
            }
            None => ctx.error(
                crate::diagnostic_messages::messages::excess_property(
                    &ctx.arena, &key.name, target,
                ),
                key.span,
            ),
            Some(target_property) => {
                check_excess_properties(&property.value, target_property.type_id, ctx);
            }
        }
    }
}
