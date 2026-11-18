// Owns: narrowing that flows through destructured groups.

use super::*;

// `const { kind, a } = x` with `x: { kind: "a", a: A } | { kind: "b", a: B }`: `kind` and `a`
// stay tied to each other, so once `kind` is "a" the binding `a` is an `A`. Only a `const`
// pattern of plain names over a union of object types is recorded.
pub fn register_destructured_group(
    object: &oxc_ast::ast::ObjectPattern,
    source: TypeId,
    ctx: &mut CheckContext<'_, '_>,
) {
    if union_members(&ctx.arena, source).is_none() {
        return;
    }
    let mut members = Vec::new();
    for property in &object.properties {
        let oxc_ast::ast::PropertyKey::StaticIdentifier(key) = &property.key else {
            continue;
        };
        let oxc_ast::ast::BindingPattern::BindingIdentifier(id) = &property.value else {
            continue;
        };
        if let Some(symbol) = id.symbol_id.get() {
            members.push((symbol, key.name.to_string()));
        }
    }
    if members.len() < 2 {
        return;
    }
    let group = ctx.destructured_groups.len();
    for (symbol, property) in &members {
        ctx.destructured_index
            .insert(*symbol, (group, property.clone()));
    }
    ctx.destructured_groups
        .push(crate::bridge::context::DestructuredGroup { source, members });
}

// For each narrowed binding that came out of a registered destructuring, narrows its
// siblings to what the members of the source that can still hold the new type say.
pub(super) fn propagate_destructured(
    ctx: &mut CheckContext<'_, '_>,
    state: NarrowState,
) -> NarrowState {
    if ctx.destructured_index.is_empty() {
        return state;
    }
    let mut out = state.clone();
    for &(symbol, narrowed) in &state.0 {
        let Some((group_index, property)) = ctx.destructured_index.get(&symbol).cloned() else {
            continue;
        };
        let source = ctx.destructured_groups[group_index].source;
        let siblings = ctx.destructured_groups[group_index].members.clone();
        let Some(members) = union_members(&ctx.arena, source) else {
            continue;
        };
        let property_of = |arena: &TypeArena, member: TypeId, name: &str| match arena.get(member) {
            Type::Object(object) => object
                .properties
                .iter()
                .find(|entry| &*entry.name == name)
                .map(|entry| entry.type_id),
            _ => None,
        };
        let mut kept = Vec::new();
        for member in members {
            let Some(discriminant) = property_of(&ctx.arena, member, &property) else {
                kept.push(member);
                continue;
            };
            let parts =
                union_members(&ctx.arena, discriminant).unwrap_or_else(|| vec![discriminant]);
            let overlaps = parts
                .iter()
                .any(|&part| crate::subtyping::is_subtype(&ctx.arena, part, narrowed));
            if overlaps {
                kept.push(member);
            }
        }
        for (other, name) in siblings {
            if other == symbol {
                continue;
            }
            let mut parts = Vec::new();
            let mut complete = true;
            for &member in &kept {
                match property_of(&ctx.arena, member, &name) {
                    Some(property_type) => parts.push(property_type),
                    None => complete = false,
                }
            }
            if !complete {
                continue;
            }
            let narrowed_type = ctx.arena.alloc_union(parts);
            out.insert(other, narrowed_type);
        }
    }
    out
}
