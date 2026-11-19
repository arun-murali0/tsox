//! `Record<K, V>` reduction.
use super::*;

/// `Record<K, V>` once K is known: one required property per literal key, or an index
/// signature when the key is `string` or `number`. `None` while K is still generic.
pub fn reduce_record(arena: &mut TypeArena, key: TypeId, value: TypeId) -> Option<TypeId> {
    fn members(arena: &TypeArena, id: TypeId, out: &mut Vec<TypeId>) {
        match arena.get(id) {
            Type::Union(parts) => {
                for &part in parts {
                    members(arena, part, out);
                }
            }
            _ => out.push(id),
        }
    }
    let mut parts = Vec::new();
    members(arena, key, &mut parts);
    let has = |arena: &TypeArena, wanted: fn(&Type) -> bool| {
        parts.iter().any(|&part| wanted(arena.get(part)))
    };
    let index = |name: &str, value: TypeId| crate::types::IndexSignature {
        key_name: name.into(),
        value,
        readonly: false,
    };
    if has(arena, |t| matches!(t, Type::String)) {
        let object = ObjectType::new(Vec::new()).with_indexes(Some(index("x", value)), None);
        return Some(arena.alloc(Type::Object(object)));
    }
    if has(arena, |t| matches!(t, Type::Number)) {
        let object = ObjectType::new(Vec::new()).with_indexes(None, Some(index("x", value)));
        return Some(arena.alloc(Type::Object(object)));
    }
    let names = mapped_key_names(arena, key)?;
    let properties = names
        .into_iter()
        .map(|name| PropertyEntry {
            order: Default::default(),
            name: name.into(),
            type_id: value,
            optional: false,
            is_method: false,
            modifiers: Default::default(),
        })
        .collect();
    Some(arena.alloc(Type::Object(ObjectType::new(properties))))
}
