//! Reduction of mapped types.
use super::*;

/// The names a mapped type's constraint stands for, or `None` while it is generic or is
/// not a plain set of literal keys (`string`, `number`).
pub(super) fn mapped_key_names(arena: &TypeArena, constraint: TypeId) -> Option<Vec<String>> {
    fn collect(arena: &TypeArena, id: TypeId, out: &mut Vec<String>) -> Option<()> {
        match arena.get(id) {
            Type::StringLiteral(name) => {
                if !out.contains(name) {
                    out.push(name.clone());
                }
            }
            Type::NumberLiteral(n) => {
                let name = if n.fract() == 0.0 {
                    format!("{n:.0}")
                } else {
                    n.to_string()
                };
                if !out.contains(&name) {
                    out.push(name);
                }
            }
            Type::Union(members) => {
                for &member in members {
                    collect(arena, member, out)?;
                }
            }
            Type::Never => {}
            _ => return None,
        }
        Some(())
    }
    let mut names = Vec::new();
    collect(arena, constraint, &mut names)?;
    Some(names)
}

pub(super) fn without_undefined(arena: &mut TypeArena, id: TypeId) -> TypeId {
    match arena.get(id).clone() {
        Type::Union(members) => {
            let kept: Vec<TypeId> = members
                .into_iter()
                .filter(|&m| !matches!(arena.get(m), Type::Undefined))
                .collect();
            match kept.len() {
                0 => arena.never(),
                1 => kept[0],
                _ => arena.alloc_union(kept),
            }
        }
        Type::Undefined => arena.never(),
        _ => id,
    }
}

/// Expands `{ [K in constraint]: value }` into an object once the constraint is a known
/// set of keys; until then the symbolic node is kept. A homomorphic one over a tuple or an
/// array maps the elements instead.
pub fn reduce_mapped(arena: &mut TypeArena, mapped: crate::types::MappedType) -> TypeId {
    use crate::types::MappedModifier;
    let generic = |arena: &TypeArena, id: TypeId| {
        free_generic_params(arena, id)
            .iter()
            .any(|(param, _)| *param != mapped.key)
    };
    if generic(arena, mapped.constraint)
        || mapped.source.is_some_and(|source| generic(arena, source))
    {
        return arena.alloc(Type::Mapped(mapped));
    }

    // A homomorphic mapped type over a primitive is that primitive.
    if let Some(source) = mapped.source
        && mapped.name_type.is_none()
        && matches!(
            arena.get(source),
            Type::Number
                | Type::String
                | Type::Boolean
                | Type::Null
                | Type::Undefined
                | Type::Void
                | Type::StringLiteral(_)
                | Type::NumberLiteral(_)
                | Type::BooleanLiteral(_)
        )
    {
        return source;
    }

    // A homomorphic mapped type over a tuple or an array maps each element.
    if let Some(source) = mapped.source
        && mapped.name_type.is_none()
    {
        match arena.get(source).clone() {
            Type::Array(element) => {
                let number = arena.alloc(Type::Number);
                let _ = element;
                let value =
                    substitute_bound_type_params(arena, mapped.value, &[(mapped.key, number)]);
                return arena.alloc(Type::Array(value));
            }
            Type::Tuple(elements) => {
                let mut mapped_elements = Vec::with_capacity(elements.len());
                for (position, element) in elements.iter().enumerate() {
                    let key = arena.alloc(Type::NumberLiteral(position as f64));
                    let value =
                        substitute_bound_type_params(arena, mapped.value, &[(mapped.key, key)]);
                    mapped_elements.push(crate::types::TupleElement {
                        type_id: value,
                        optional: match mapped.optional {
                            MappedModifier::Add => true,
                            MappedModifier::Remove => false,
                            MappedModifier::Keep => element.optional,
                        },
                        rest: element.rest,
                    });
                }
                return arena.alloc_tuple(mapped_elements);
            }
            _ => {}
        }
    }

    if mapped.name_type.is_none()
        && matches!(arena.get(mapped.constraint), Type::String | Type::Number)
    {
        let is_string = matches!(arena.get(mapped.constraint), Type::String);
        let key = mapped.constraint;
        let value = substitute_bound_type_params(arena, mapped.value, &[(mapped.key, key)]);
        let signature = crate::types::IndexSignature {
            key_name: "key".into(),
            value,
            readonly: false,
        };
        let object = ObjectType::new(Vec::new());
        return arena.alloc(Type::Object(if is_string {
            object.with_indexes(Some(signature), None)
        } else {
            object.with_indexes(None, Some(signature))
        }));
    }

    let Some(names) = mapped_key_names(arena, mapped.constraint) else {
        return arena.alloc(Type::Mapped(mapped));
    };
    let source_properties = mapped.source.and_then(|source| match arena.get(source) {
        Type::Object(object) => Some(object.properties.clone()),
        _ => None,
    });
    let mut properties: Vec<PropertyEntry> = Vec::with_capacity(names.len());
    for name in names {
        let key = arena.alloc(Type::StringLiteral(name.clone()));
        let original = source_properties
            .as_ref()
            .and_then(|all| all.iter().find(|p| &*p.name == name.as_str()));
        // `as` renames the key (or drops it with `never`).
        let final_names: Vec<String> = match mapped.name_type {
            None => vec![name.clone()],
            Some(name_type) => {
                let renamed = substitute_bound_type_params(arena, name_type, &[(mapped.key, key)]);
                match mapped_key_names(arena, renamed) {
                    Some(names) => names,
                    None => return arena.alloc(Type::Mapped(mapped)),
                }
            }
        };
        let mut value = substitute_bound_type_params(arena, mapped.value, &[(mapped.key, key)]);
        let optional = match mapped.optional {
            MappedModifier::Add => true,
            MappedModifier::Remove => false,
            MappedModifier::Keep => original.is_some_and(|p| p.optional),
        };
        if mapped.optional == MappedModifier::Remove && original.is_some_and(|p| p.optional) {
            value = without_undefined(arena, value);
        }
        for final_name in final_names {
            properties.push(PropertyEntry {
                order: Default::default(),
                name: final_name.into(),
                type_id: value,
                optional,
                is_method: false,
                modifiers: crate::types::PropModifiers {
                    readonly: match mapped.readonly {
                        MappedModifier::Add => true,
                        MappedModifier::Remove => false,
                        MappedModifier::Keep => original.is_some_and(|p| p.modifiers.readonly),
                    },
                    ..Default::default()
                },
            });
        }
    }
    arena.alloc(Type::Object(ObjectType::new(properties)))
}
