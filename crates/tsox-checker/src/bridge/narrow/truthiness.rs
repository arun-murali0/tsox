// Owns: narrowing by truthiness and by null/undefined checks, and the `narrow_to_*` helpers.

use super::*;

pub(super) fn narrow_by_nullish(
    arena: &mut TypeArena,
    id: TypeId,
    target_is_null: bool,
    is_loose: bool,
    want_match: bool,
) -> TypeId {
    let is_target = |arena: &TypeArena, t: TypeId| {
        // `==`/`!=` against either `null` or `undefined` matches both: this is
        // JavaScript's specific loose-equality-with-null quirk, not general
        // loose equality, and it is symmetric: `x == undefined` matches a
        // `null` value too, the same as `x == null` does. `===`/`!==` only ever
        // matches the exact literal on the right, so target_is_null still
        // decides which single type counts there.
        if is_loose {
            matches!(arena.get(t), Type::Null | Type::Undefined)
        } else if target_is_null {
            matches!(arena.get(t), Type::Null)
        } else {
            matches!(arena.get(t), Type::Undefined)
        }
    };
    // Problem: `any` and `unknown` are not null and not undefined, so both branches
    // of `x === null` filtered them out and `x` became never; a read of `x.foo` in
    // the true branch then reported a property missing on never.
    // Picked: a type that can hold anything (any, unknown), the error sentinel and a
    // type parameter are kept in both branches, because the test cannot rule them
    // in or out.
    // Cost: the true branch keeps `unknown` as unknown where tsc would say null.
    let is_open = |arena: &TypeArena, t: TypeId| {
        matches!(
            arena.get(t),
            Type::Any | Type::Unknown | Type::Error | Type::GenericParameter(..)
        )
    };
    // A union that still holds `unknown` (what joining `null` with `unknown` leaves) is no
    // narrower than `unknown`.
    let id = match union_members(arena, id) {
        Some(members)
            if members
                .iter()
                .any(|&m| matches!(arena.get(m), Type::Unknown)) =>
        {
            arena.unknown()
        }
        _ => id,
    };
    match union_members(arena, id) {
        Some(members) => {
            let filtered = members
                .into_iter()
                .filter(|&m| is_open(arena, m) || is_target(arena, m) == want_match)
                .collect();
            arena.alloc_union(filtered)
        }
        None => {
            // `unknown` is exactly what the test says it is where the test passes.
            if want_match && matches!(arena.get(id), Type::Unknown) {
                let null = arena.null();
                let undefined = arena.undefined();
                return if is_loose {
                    arena.alloc_union(vec![null, undefined])
                } else if target_is_null {
                    null
                } else {
                    undefined
                };
            }
            if is_open(arena, id) || is_target(arena, id) == want_match {
                id
            } else {
                arena.never()
            }
        }
    }
}

// Conservative on purpose: only returns true for a type that is provably falsy in
// every case, such as the literal 0 or an empty string literal. A plain, un-narrowed
// number or string is neither definitely falsy nor definitely truthy, so it is left
// alone by narrow_truthy below rather than guessed at.
pub(super) fn is_definitely_falsy(arena: &TypeArena, id: TypeId) -> bool {
    match arena.get(id) {
        Type::Null | Type::Undefined | Type::BooleanLiteral(false) => true,
        Type::NumberLiteral(n) => *n == 0.0,
        Type::StringLiteral(s) => s.is_empty(),
        _ => false,
    }
}

pub(super) fn is_definitely_truthy(arena: &TypeArena, id: TypeId) -> bool {
    match arena.get(id) {
        Type::Object(_)
        | Type::Array(_)
        | Type::Tuple(_)
        | Type::Function(_)
        | Type::BooleanLiteral(true) => true,
        Type::NumberLiteral(n) => *n != 0.0,
        Type::StringLiteral(s) => !s.is_empty(),
        _ => false,
    }
}

// Keeps what can still be truthy (or falsy). Only a member that is certainly the other
// kind is dropped, so a doubt keeps the member: a narrowing may lose precision and must
// never invent it.
pub(super) fn narrow_truthy(arena: &mut TypeArena, id: TypeId, want_truthy: bool) -> TypeId {
    match union_members(arena, id) {
        Some(members) => {
            let filtered = members
                .into_iter()
                .filter(|&m| {
                    if want_truthy {
                        !is_definitely_falsy(arena, m)
                    } else {
                        !is_definitely_truthy(arena, m)
                    }
                })
                .collect();
            arena.alloc_union(filtered)
        }
        None => {
            let keep = if want_truthy {
                !is_definitely_falsy(arena, id)
            } else {
                !is_definitely_truthy(arena, id)
            };
            if keep { id } else { arena.never() }
        }
    }
}

// Narrows a union of object types by whether one of their properties is truthy. Only a
// union narrows; a member that is not an object, or has no such property, is kept. A
// member stays on the truthy side unless its property is always falsy, and on the falsy
// side unless its property is always truthy, using the same conservative notions as
// narrow_truthy: a plain `number` or `string` can be either, so it is never dropped.
pub(super) fn narrow_by_property_truthiness(
    arena: &mut TypeArena,
    id: TypeId,
    property: &str,
    want_truthy: bool,
) -> TypeId {
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
                candidates.iter().any(|&candidate| {
                    if want_truthy {
                        !is_definitely_falsy(arena, candidate)
                    } else {
                        !is_definitely_truthy(arena, candidate)
                    }
                })
            }
        };
        if keep {
            kept.push(member);
        }
    }
    arena.alloc_union(kept)
}

pub(super) fn union_members(arena: &TypeArena, id: TypeId) -> Option<Vec<TypeId>> {
    match arena.get(id) {
        Type::Union(members) => Some(members.clone()),
        _ => None,
    }
}

pub fn narrow_to_truthy(arena: &mut TypeArena, id: TypeId) -> TypeId {
    narrow_truthy(arena, id, true)
}

pub fn narrow_to_falsy(arena: &mut TypeArena, id: TypeId) -> TypeId {
    narrow_truthy(arena, id, false)
}

// Takes `null` and `undefined` out of a type: the value that is left once the nullish case
// is ruled out. It is the left side of `??`, the result of `!`, a bound name that has a
// default, and the operand of a read or an operation after the nullish error was already
// reported. A lone `null` or `undefined` leaves `never`, which the caller reads as nothing
// left to use.
pub fn narrow_to_non_nullish(arena: &mut TypeArena, id: TypeId) -> TypeId {
    let is_nullish =
        |arena: &TypeArena, t: TypeId| matches!(arena.get(t), Type::Null | Type::Undefined);
    match union_members(arena, id) {
        Some(members) => {
            let filtered = members
                .into_iter()
                .filter(|&m| !is_nullish(arena, m))
                .collect();
            arena.alloc_union(filtered)
        }
        None => {
            if is_nullish(arena, id) {
                arena.never()
            } else {
                id
            }
        }
    }
}
