// Owns: which `typeof` tags exist and how a tag filters a type.

use super::*;

pub(super) fn is_known_typeof_tag(tag: &str) -> bool {
    matches!(
        tag,
        "string" | "number" | "boolean" | "undefined" | "object" | "function"
    )
}

// `typeof null` is "object" in JavaScript, so null belongs with objects and
// arrays here rather than with the primitives, and a `typeof x === "object"`
// check therefore does not remove null from x.
pub(super) fn matches_typeof_tag(arena: &TypeArena, id: TypeId, tag: &str) -> bool {
    matches!(
        (arena.get(id), tag),
        (Type::String | Type::StringLiteral(_), "string")
            | (Type::Number | Type::NumberLiteral(_), "number")
            | (Type::Boolean | Type::BooleanLiteral(_), "boolean")
            | (Type::Undefined, "undefined")
            | (
                Type::Object(_) | Type::Array(_) | Type::Tuple(_) | Type::Null,
                "object"
            )
            | (Type::Function(_), "function")
    )
}

// `unknown` becomes the matching primitive in the true branch (`typeof x ===
// "string"` makes an unknown `x` a string) and stays unknown otherwise. `any`, the
// error type and a generic parameter are kept as they are on both sides: narrowing
// them to never would turn a gap in this checker's model into errors on valid
// source. A lone (non-union) type is handled as a one-member union, so one that
// does not match still ends up as never.
pub(super) fn narrow_by_typeof(
    arena: &mut TypeArena,
    id: TypeId,
    tag: &str,
    want_match: bool,
) -> TypeId {
    let members = union_members(arena, id).unwrap_or_else(|| vec![id]);
    let mut kept = Vec::with_capacity(members.len());
    for member in members {
        // A type parameter that `typeof x === "number"` matched is a `T & number` (tsc), unless its
        // bound already is a number. Excluded, it stays `T`.
        if want_match
            && let Type::GenericParameter(_, _, constraint) = *arena.get(member)
            && let Some(primitive) = primitive_for_tag(arena, tag)
        {
            let already = constraint
                .is_some_and(|bound| crate::subtyping::is_subtype(arena, bound, primitive));
            match (already, arena.alloc_intersection(vec![member, primitive])) {
                (false, Ok(narrowed)) => kept.push(narrowed),
                _ => kept.push(member),
            }
            continue;
        }
        let opaque = matches!(
            arena.get(member),
            Type::Any | Type::Error | Type::GenericParameter(..)
        );
        let is_unknown = matches!(arena.get(member), Type::Unknown);
        if opaque {
            kept.push(member);
        } else if is_unknown {
            if want_match {
                kept.push(primitive_for_tag(arena, tag).unwrap_or(member));
            } else {
                kept.push(member);
            }
        } else if matches_typeof_tag(arena, member, tag) == want_match {
            kept.push(member);
        }
    }
    arena.alloc_union(kept)
}

pub(super) fn primitive_for_tag(arena: &TypeArena, tag: &str) -> Option<TypeId> {
    match tag {
        "string" => Some(arena.string()),
        "number" => Some(arena.number()),
        "boolean" => Some(arena.boolean()),
        "undefined" => Some(arena.undefined()),
        _ => None,
    }
}
