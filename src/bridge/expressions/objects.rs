use oxc_ast::ast::{ObjectPropertyKind, PropertyKey, PropertyKind};
use oxc_semantic::Scoping;

use crate::arena::TypeId;
use crate::types::{ObjectType, PropertyEntry, Type};

use super::super::context::CheckContext;
use super::infer_expression_type;

// A computed key is silently skipped rather than making the whole object literal
// unresolvable. This is the opposite tradeoff from an interface or class
// declaration (see namespace.rs), where one unsupported member makes the whole
// declaration unresolvable: a value expression's inferred type is used immediately
// at its own use site, so under-describing it here is safer than the alternative
// of refusing to type the whole expression.
//
// A spread merges the spread object's properties in source order, so a later key
// replaces an earlier one, the same as at runtime. A spread whose type cannot be
// read as a plain object (any, an error, a union, an array, a Record) makes the
// whole literal's shape unknowable, so it is typed as the error sentinel, which is
// compatible with everything, instead of guessing a partial shape that would
// produce false "property does not exist" errors. Spreading null or undefined adds
// nothing and is fine.
//
// Problem: every property was widened as soon as it was read, so `{ tag: 'x' }` became
// `{ tag: string }` and was then rejected against `tag: 'x'`, an error tsc does not give.
// Picked: when the caller knows the declared type (`expected`), a property whose declared
// type holds a literal keeps its literal, and the declared property type is handed down so
// a nested object or array is read the same way. Everything else widens as before.
// Cost: only an annotated variable, a return, a call argument, an assignment and the
// literals nested in those get this; other positions still widen.
pub(super) fn infer_object_expression_type(
    object: &oxc_ast::ast::ObjectExpression,
    expected: Option<TypeId>,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> TypeId {
    let mut properties: Vec<PropertyEntry> = Vec::with_capacity(object.properties.len());
    let mut shape_unknown = false;

    for property in &object.properties {
        let property = match property {
            ObjectPropertyKind::ObjectProperty(property) => property,
            ObjectPropertyKind::SpreadProperty(spread) => {
                let spread_type = infer_expression_type(&spread.argument, scoping, ctx);
                match ctx.arena.get(spread_type).clone() {
                    Type::Object(spread_object)
                        if ctx.arena.record_value_type(spread_type).is_none() =>
                    {
                        for entry in spread_object.properties.iter() {
                            upsert_property(&mut properties, entry.clone());
                        }
                    }
                    Type::Null | Type::Undefined => {}
                    _ => shape_unknown = true,
                }
                continue;
            }
        };
        // A key can be a plain identifier (`{ a: 1 }`), a string literal
        // (`{ "a": 1 }`), or a numeric literal (`{ 1: "x" }`); JavaScript stores
        // all three as string-keyed properties, so a numeric key is formatted the
        // same way its runtime string form would be. Anything else, a computed
        // key (`{ [expr]: 1 }`) or a private name, has no static name to give
        // this property, so it is skipped rather than guessed at.
        let name = match &property.key {
            PropertyKey::StaticIdentifier(key) => key.name.to_string(),
            PropertyKey::StringLiteral(key) => key.value.to_string(),
            PropertyKey::NumericLiteral(key) => key.value.to_string(),
            _ => continue,
        };
        let declared = expected
            .map(|e| declared_properties(ctx, e, &name))
            .unwrap_or_default();
        // Several members of a union can declare the property differently; the literal is
        // kept if any of them holds one, and a single declaration is also the context a
        // nested literal is read against.
        ctx.contextual = if declared.len() == 1 {
            Some(declared[0])
        } else {
            None
        };
        let type_id = infer_expression_type(&property.value, scoping, ctx);
        // `{ get x() { ... } }` has a property x of the type the getter returns, and a setter
        // one of the type it accepts. The function carrying either is not the property.
        // A getter wins over a setter of the same name, whichever comes first.
        let type_id = if property.kind == PropertyKind::Init {
            if declared.iter().any(|d| holds_literal(ctx, *d)) {
                type_id
            } else {
                crate::types::widen(&ctx.arena, type_id)
            }
        } else {
            if property.kind == PropertyKind::Set && properties.iter().any(|p| &*p.name == name) {
                continue;
            }
            match accessor_value_type(ctx, property.kind, type_id) {
                Some(value_type) => value_type,
                None => continue,
            }
        };
        upsert_property(
            &mut properties,
            PropertyEntry {
                name: name.into(),
                type_id,
                optional: false,
                // `{ peek() {} }` prints as `{ peek(): any; }`, the way it was written.
                is_method: property.method,
            },
        );
    }

    if shape_unknown {
        return ctx.arena.error();
    }
    ctx.arena.alloc(Type::Object(ObjectType::new(properties)))
}

// The type an accessor stands for: what a getter returns, what a setter takes first.
fn accessor_value_type(
    ctx: &CheckContext<'_, '_>,
    kind: PropertyKind,
    function: TypeId,
) -> Option<TypeId> {
    let Type::Function(function) = ctx.arena.get(function) else {
        return None;
    };
    match kind {
        PropertyKind::Get => Some(function.return_type),
        PropertyKind::Set => function.params.first().map(|param| param.type_id),
        PropertyKind::Init => None,
    }
}

// The declared types of property `name` in `expected`: an object has it directly, a union
// of objects has one per member that declares it, and anything else has none.
fn declared_properties(ctx: &CheckContext<'_, '_>, expected: TypeId, name: &str) -> Vec<TypeId> {
    let mut found: Vec<TypeId> = Vec::new();
    let members: Vec<TypeId> = match ctx.arena.get(expected) {
        Type::Union(members) => members.clone(),
        _ => vec![expected],
    };
    for member in members {
        if let Type::Object(object) = ctx.arena.get(member) {
            if let Some(property) = object.properties.iter().find(|p| &*p.name == name) {
                if !found.contains(&property.type_id) {
                    found.push(property.type_id);
                }
            }
        }
    }
    found
}

// Whether a declared type is, or is a union containing, a literal type.
pub(super) fn holds_literal(ctx: &CheckContext<'_, '_>, declared: TypeId) -> bool {
    match ctx.arena.get(declared) {
        Type::StringLiteral(_) | Type::NumberLiteral(_) | Type::BooleanLiteral(_) => true,
        Type::Union(members) => members.iter().any(|m| holds_literal(ctx, *m)),
        _ => false,
    }
}

// Keeps names unique, which ObjectType's sorted merge-join relies on: a repeated
// key (`{ a: 1, a: 2 }`, or a spread followed by an explicit key) replaces the
// earlier entry in place rather than adding a second one.
fn upsert_property(properties: &mut Vec<PropertyEntry>, entry: PropertyEntry) {
    match properties.iter_mut().find(|p| p.name == entry.name) {
        Some(existing) => *existing = entry,
        None => properties.push(entry),
    }
}

// Each element's type is widened (a literal 5 becomes number) before being added
// to the element-type union, matching how [1, 2, 3] should infer as number[], not
// a union of three numeric literal types. An empty array literal has nothing to
// infer an element type from, so it is never[]: the type with no elements, assignable to
// an array of anything, which is what makes `const xs: number[] = []`, `return []` and
// `f([])` valid. A variable declared without an annotation widens it to any[] (see
// types::widen), because tsc treats `const xs = []` as an array that fills up later.
pub(super) fn infer_array_expression_type(
    array: &oxc_ast::ast::ArrayExpression,
    expected: Option<TypeId>,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> TypeId {
    let mut element_types = Vec::with_capacity(array.elements.len());

    for element in &array.elements {
        // Problem: `[...xs, 1]` ignored the spread, so the element type left out
        // whatever xs held and a later assignment was checked against a type that
        // was too narrow.
        // Picked: a spread of an array contributes that array's element type. A
        // spread of anything else (any, an error, a tuple-like or iterable type this
        // checker does not model) contributes any, which keeps the result usable
        // without guessing a shape.
        // Cost: iterables such as a Set or a string spread to any, not their item type.
        if let oxc_ast::ast::ArrayExpressionElement::SpreadElement(spread) = element {
            let spread_type = infer_expression_type(&spread.argument, scoping, ctx);
            let item = match ctx.arena.get(spread_type) {
                Type::Array(item) => *item,
                _ => ctx.arena.any(),
            };
            let item = crate::types::widen(&ctx.arena, item);
            if !element_types.contains(&item) {
                element_types.push(item);
            }
            continue;
        }
        let Some(expr) = element.as_expression() else {
            continue;
        };
        let declared_item = match expected.map(|e| ctx.arena.get(e)) {
            Some(Type::Array(item)) => Some(*item),
            _ => None,
        };
        ctx.contextual = declared_item;
        let type_id = infer_expression_type(expr, scoping, ctx);
        let type_id = match declared_item {
            Some(d) if holds_literal(ctx, d) => type_id,
            _ => crate::types::widen(&ctx.arena, type_id),
        };
        if !element_types.contains(&type_id) {
            element_types.push(type_id);
        }
    }

    let element_type = match element_types.len() {
        0 => ctx.arena.never(),
        1 => element_types[0],
        _ => ctx.arena.alloc_union(element_types),
    };

    ctx.arena.alloc(Type::Array(element_type))
}
