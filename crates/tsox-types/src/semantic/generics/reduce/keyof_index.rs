//! Reduction of `keyof T` and indexed access `T[K]`.
use super::*;

/// `keyof operand`: the union of the property names when they are all known, otherwise
/// the symbolic node (the operand is still generic, or has index signatures).
pub fn reduce_keyof(arena: &mut TypeArena, operand: TypeId) -> TypeId {
    // Record<K, V> is an opaque application while K is generic, but its key set is
    // exactly K. TypeArena::get exposes its empty implementation body, so asking
    // concrete_keys first would incorrectly turn keyof Record<K, V> into never.
    if let Some(keys) = arena.record_key_type(operand) {
        return keys;
    }
    if let Some(keys) = crate::type_annotation::concrete_keys(arena, operand) {
        let members: Vec<TypeId> = keys
            .iter()
            .map(|key| arena.alloc(Type::StringLiteral(key.to_string())))
            .collect();
        return match members.len() {
            0 => arena.never(),
            1 => members[0],
            _ => arena.alloc_union(members),
        };
    }
    if let Type::Object(object) = arena.get(operand).clone()
        && (object.string_index.is_some() || object.number_index.is_some())
    {
        let mut members: Vec<TypeId> = Vec::new();
        if object.string_index.is_some() {
            members.push(arena.alloc(Type::String));
            members.push(arena.alloc(Type::Number));
        } else {
            members.push(arena.alloc(Type::Number));
        }
        for property in object.properties.iter() {
            if object.string_index.is_none() {
                members.push(arena.alloc(Type::StringLiteral(property.name.to_string())));
            }
        }
        return if members.len() == 1 {
            members[0]
        } else {
            arena.alloc_union(members)
        };
    }
    arena.alloc(Type::Keyof(operand))
}

/// `object[index]`: the property's type when both sides are concrete, otherwise the
/// symbolic node.
pub fn reduce_indexed_access(arena: &mut TypeArena, object: TypeId, index: TypeId) -> TypeId {
    if matches!(arena.get(index), Type::Never) {
        return arena.never();
    }
    // For a generic Record the application remains opaque until substitution, but the
    // relation `Record<K, V>[K]` is already known: every key in K is present and has V.
    // A wider index type must remain unresolved rather than borrowing V for an invalid key.
    if let (Some(key_domain), Some(value)) = (
        arena.record_key_type(object),
        arena.record_value_type(object),
    ) && crate::subtyping::is_subtype(arena, index, key_domain)
    {
        return value;
    }
    if !contains_type_param(arena, object)
        && !contains_type_param(arena, index)
        && let Some(reduced) = crate::type_annotation::indexed_access(arena, object, index)
    {
        return reduced;
    }
    arena.alloc(Type::IndexedAccess(object, index))
}
