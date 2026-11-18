// Owns: narrowing by `"k" in x` and by the presence of a property across union members.

use super::*;

// `"radius" in shape`: narrows a union of object types by whether a member has the
// property. Only a literal key and a plain identifier are recognised.
pub(super) fn narrow_by_in(
    bin: &BinaryExpression,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> (NarrowState, NarrowState) {
    // A string or numeric literal key: `"a" in x`, `1 in x` (the property `1`).
    let property = match &bin.left {
        Expression::StringLiteral(key) => key.value.to_string(),
        Expression::NumericLiteral(key) if key.value.fract() == 0.0 && key.value.abs() < 1e15 => {
            format!("{}", key.value as i64)
        }
        _ => return empty_pair(),
    };
    let Expression::Identifier(ident) = &bin.right else {
        return empty_pair();
    };
    let Some(symbol_id) = resolve_symbol_id(ident, scoping) else {
        return empty_pair();
    };
    by_symbol(ctx, symbol_id, move |arena, current, want_present| {
        narrow_by_property_presence(arena, current, &property, want_present)
    })
}

// How a union member relates to a property name, as far as `in` can tell.
pub(super) enum Presence {
    Required,
    Optional,
    Absent,
    // Not an object, or a Record whose generic key domain is not represented as a
    // concrete property list: it may or may not have the property, so `in` can neither
    // keep nor drop it with confidence.
    Unknown,
}

// Absent is only said of an object that lists its properties. A non-object, and a Record
// (its keys come from K, not from a concrete property list while K is generic), read as
// Unknown, so neither branch drops them.
pub(super) fn property_presence(arena: &TypeArena, member: TypeId, property: &str) -> Presence {
    // An intersection has the property when any part has it.
    if let Type::Intersection(parts) = arena.get(member) {
        let parts: Vec<Presence> = parts
            .iter()
            .map(|&part| property_presence(arena, part, property))
            .collect();
        return if parts.iter().any(|p| matches!(p, Presence::Required)) {
            Presence::Required
        } else if parts.iter().any(|p| matches!(p, Presence::Optional)) {
            Presence::Optional
        } else if parts.iter().all(|p| matches!(p, Presence::Absent)) {
            Presence::Absent
        } else {
            Presence::Unknown
        };
    }
    let Type::Object(object) = arena.get(member) else {
        return Presence::Unknown;
    };
    if arena.record_value_type(member).is_some() {
        return Presence::Unknown;
    }
    // An index signature may hold the property (a numeric key only with a numeric one).
    if object.string_index.is_some()
        || (object.number_index.is_some() && property.parse::<f64>().is_ok())
    {
        return Presence::Optional;
    }
    match object
        .properties
        .iter()
        .find(|entry| &*entry.name == property)
    {
        Some(entry) if entry.optional => Presence::Optional,
        Some(_) => Presence::Required,
        None => Presence::Absent,
    }
}

// The true branch keeps the members that have, or may have, the property; the false
// branch drops only the members where it is required, since an optional or missing
// property can still be absent. A lone object type is returned unchanged: tsc would
// intersect it with a record of the key, which this checker has no way to express.
// If no member could have the property the union is also left alone, for the same
// reason, rather than collapsing to never.
pub(super) fn narrow_by_property_presence(
    arena: &mut TypeArena,
    id: TypeId,
    property: &str,
    want_present: bool,
) -> TypeId {
    let members = union_members(arena, id).unwrap_or_else(|| vec![id]);
    let presences: Vec<Presence> = members
        .iter()
        .map(|&member| property_presence(arena, member, property))
        .collect();

    // No member could have the property (the check is for one the type does not declare): tsc
    // intersects the type with `Record<"name", unknown>`, so `x.name` reads as `unknown` after the
    // test. A Record with a generic key domain may have it, so that case is left alone.
    if want_present
        && presences
            .iter()
            .all(|p| matches!(p, Presence::Absent | Presence::Unknown))
        && !members.iter().any(|&member| {
            matches!(arena.get(member), Type::Object(_))
                && arena.record_value_type(member).is_some()
        })
        && !matches!(arena.get(id), Type::Any | Type::Error)
    {
        let unknown = arena.unknown();
        let record = arena.alloc(Type::Object(crate::types::ObjectType::new(vec![
            crate::types::PropertyEntry {
                name: property.into(),
                type_id: unknown,
                optional: false,
                is_method: false,
                modifiers: Default::default(),
            },
        ])));
        return arena.alloc_intersection(vec![id, record]).unwrap_or(id);
    }
    if union_members(arena, id).is_none() {
        // A lone object that requires the property cannot lack it.
        if !want_present && matches!(presences[0], Presence::Required) {
            return arena.never();
        }
        return id;
    }

    let mut kept = Vec::with_capacity(members.len());
    for (member, presence) in members.iter().zip(&presences) {
        let keep = if want_present {
            !matches!(presence, Presence::Absent)
        } else {
            !matches!(presence, Presence::Required)
        };
        if keep {
            kept.push(*member);
        }
    }
    arena.alloc_union(kept)
}
