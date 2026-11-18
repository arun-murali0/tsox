// Owns: narrowing by literal equality and by a property's literal (discriminated unions).

use super::*;

// `ident.property === <literal>` (either order) is how a discriminated union is
// narrowed. An optional chain is left out because `ident?.property` can also be
// undefined, which this narrowing does not model.
pub(super) fn member_literal_check(
    maybe_member: &Expression,
    maybe_literal: &Expression,
    scoping: &Scoping,
    arena: &mut TypeArena,
) -> Option<(SymbolId, String, TypeId)> {
    let Expression::StaticMemberExpression(member) = maybe_member else {
        return None;
    };
    if member.optional {
        return None;
    }
    let Expression::Identifier(ident) = &member.object else {
        return None;
    };
    let symbol_id = resolve_symbol_id(ident, scoping)?;
    let literal = resolve_literal(maybe_literal, arena)?;
    Some((symbol_id, member.property.name.to_string(), literal))
}

pub(super) fn narrow_by_literal(
    arena: &mut TypeArena,
    id: TypeId,
    literal: TypeId,
    want_match: bool,
) -> TypeId {
    narrow_by_literals(arena, id, &[literal], want_match)
}

// Narrows by equality against a set of literals.
// Matching direction: a wide member (`string` for "a", `number` for 1, `boolean`
// for true, `unknown` for anything) is replaced by the literals it covers, as tsc
// does. Keeping the wide member would lose the narrowing, and dropping it would
// turn `if (s === "a")` on a plain `string` into `never` and flag valid code.
// `any`, the error type and a generic parameter are kept as they are, because a
// guessed narrowing for them would turn a gap in this checker into false errors.
// Excluding direction: only an exact literal member is removed, since `string`
// minus "a" is still `string`. `boolean` is the exception because it is exactly
// true | false, so removing one leaves the other.
// Literals are compared by TypeId, which is sound only because every literal Type
// is interned.
pub(super) fn narrow_by_literals(
    arena: &mut TypeArena,
    id: TypeId,
    literals: &[TypeId],
    want_match: bool,
) -> TypeId {
    let members = union_members(arena, id).unwrap_or_else(|| vec![id]);
    let mut kept: Vec<TypeId> = Vec::with_capacity(members.len());
    for member in members {
        if want_match {
            keep_matching(arena, member, literals, &mut kept);
        } else {
            keep_remaining(arena, member, literals, &mut kept);
        }
    }
    arena.alloc_union(kept)
}

pub(super) fn literal_widens_to(arena: &TypeArena, wide: TypeId, literal: TypeId) -> bool {
    matches!(
        (arena.get(wide), arena.get(literal)),
        (Type::String, Type::StringLiteral(_))
            | (Type::Number, Type::NumberLiteral(_))
            | (Type::Boolean, Type::BooleanLiteral(_))
    )
}

// The members of a union that can still equal one of the literals. A member that is one of
// them stays. `any`, the error type and a type parameter stay because nothing is known
// against them, `unknown` becomes the literals themselves, and a primitive keeps the
// literals it widens from (`string` with "a" leaves "a").
pub(super) fn keep_matching(
    arena: &TypeArena,
    member: TypeId,
    literals: &[TypeId],
    kept: &mut Vec<TypeId>,
) {
    if literals.contains(&member) {
        kept.push(member);
        return;
    }
    match arena.get(member) {
        Type::Any | Type::Error | Type::GenericParameter(..) => kept.push(member),
        Type::Unknown => kept.extend_from_slice(literals),
        Type::String | Type::Number | Type::Boolean => kept.extend(
            literals
                .iter()
                .copied()
                .filter(|&literal| literal_widens_to(arena, member, literal)),
        ),
        _ => {}
    }
}

// The false side of the same test: the members left once the literals are ruled out.
// `boolean` is `true | false` under one id, so ruling out one literal has to leave the
// other, which is what `x !== true` must give.
pub(super) fn keep_remaining(
    arena: &mut TypeArena,
    member: TypeId,
    literals: &[TypeId],
    kept: &mut Vec<TypeId>,
) {
    if literals.contains(&member) {
        return;
    }
    let excludes_a_boolean = literals
        .iter()
        .any(|&literal| matches!(arena.get(literal), Type::BooleanLiteral(_)));
    if matches!(arena.get(member), Type::Boolean) && excludes_a_boolean {
        for value in [true, false] {
            let excluded = literals.iter().any(
                |&literal| matches!(arena.get(literal), Type::BooleanLiteral(b) if *b == value),
            );
            if !excluded {
                kept.push(arena.alloc(Type::BooleanLiteral(value)));
            }
        }
        return;
    }
    kept.push(member);
}

// Narrows a union of object types by one property's literal type (the
// discriminated-union case). A lone object type is returned unchanged, since there
// is nothing to discriminate between. A member that is not an object, or lacks the
// property, is kept: this cannot prove it does not match, and dropping a member
// that does match would report valid code as an error. Matching keeps a member
// whose property type can be one of the literals; excluding removes a member only
// when its property type is exactly one literal, because a property typed
// "a" | "b" could still be "b".
pub(super) fn narrow_by_property_literals(
    arena: &mut TypeArena,
    id: TypeId,
    property: &str,
    literals: &[TypeId],
    want_match: bool,
) -> TypeId {
    // A type parameter constrained to a union (`T extends A | B`) narrows through its constraint:
    // tsc reads `T & A` there, and the members of `A` answer the property reads that follow.
    let id = match arena.get(id) {
        Type::GenericParameter(_, _, Some(constraint))
            if union_members(arena, *constraint).is_some() =>
        {
            *constraint
        }
        _ => id,
    };
    let Some(members) = union_members(arena, id) else {
        return id;
    };
    let mut kept = Vec::with_capacity(members.len());
    for member in members {
        let property_type = match arena.get(member) {
            Type::Object(object) => object
                .properties
                .iter()
                .find(|entry| &*entry.name == property)
                .map(|entry| entry.type_id),
            _ => None,
        };
        let keep = match property_type {
            None => true,
            Some(property_type) => {
                let candidates =
                    union_members(arena, property_type).unwrap_or_else(|| vec![property_type]);
                if want_match {
                    candidates.iter().any(|&candidate| {
                        literals
                            .iter()
                            .any(|&literal| property_may_equal(arena, candidate, literal))
                    })
                } else {
                    !(candidates.len() == 1 && literals.contains(&candidates[0]))
                }
            }
        };
        if keep {
            kept.push(member);
        }
    }
    arena.alloc_union(kept)
}

pub(super) fn property_may_equal(arena: &TypeArena, candidate: TypeId, literal: TypeId) -> bool {
    if candidate == literal {
        return true;
    }
    // matches! rather than a match with `=> true` arms: every arm is a bare bool,
    // which clippy::match_like_matches_macro rejects under -D warnings.
    matches!(
        (arena.get(candidate), arena.get(literal)),
        (
            Type::Any | Type::Unknown | Type::Error | Type::GenericParameter(..),
            _
        ) | (Type::String, Type::StringLiteral(_))
            | (Type::Number, Type::NumberLiteral(_))
            | (Type::Boolean, Type::BooleanLiteral(_))
    )
}
