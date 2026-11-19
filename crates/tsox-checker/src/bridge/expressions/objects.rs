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
            if ctx.const_context || declared.iter().any(|d| holds_literal(ctx, *d)) {
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
                order: Default::default(),
                name: name.into(),
                type_id,
                optional: false,
                // `{ peek() {} }` prints as `{ peek(): any; }`, the way it was written.
                is_method: property.method,
                modifiers: crate::types::PropModifiers {
                    readonly: ctx.const_context,
                    ..Default::default()
                },
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
    fn collect(
        ctx: &CheckContext<'_, '_>,
        id: TypeId,
        name: &str,
        depth: u8,
        found: &mut Vec<TypeId>,
    ) {
        if depth > 4 {
            return;
        }
        match ctx.arena.get(id) {
            // Each member of a union may declare the property, and each member of an
            // intersection does too: a literal written for `{ a: 1 } & { b: 2 }` is read
            // against both.
            Type::Union(members) | Type::Intersection(members) => {
                for &member in members.clone().iter() {
                    collect(ctx, member, name, depth + 1, found);
                }
            }
            Type::Object(object) => {
                if let Some(property) = object.properties.iter().find(|p| &*p.name == name)
                    && !found.contains(&property.type_id)
                {
                    found.push(property.type_id);
                }
            }
            _ => {}
        }
    }
    let mut found: Vec<TypeId> = Vec::new();
    collect(ctx, expected, name, 0, &mut found);
    found
}

// Whether a declared type is, or is a union containing, a literal type.
pub(super) fn holds_literal(ctx: &CheckContext<'_, '_>, declared: TypeId) -> bool {
    match ctx.arena.get(declared) {
        Type::StringLiteral(_) | Type::NumberLiteral(_) | Type::BooleanLiteral(_) => true,
        Type::Union(members) => members.iter().any(|m| holds_literal(ctx, *m)),
        // A type parameter whose constraint holds a literal or a primitive (`T extends "a" | "b"`,
        // `K extends string`) asks for the literal too: tsc keeps `"1"` in `f({ value: "1" })`
        // against `{ value: T }`, so that T can be inferred as `"1"` and not widened to string.
        Type::GenericParameter(_, _, Some(bound)) => {
            holds_literal(ctx, *bound) || holds_primitive(ctx, *bound)
        }
        _ => false,
    }
}

fn holds_primitive(ctx: &CheckContext<'_, '_>, declared: TypeId) -> bool {
    match ctx.arena.get(declared) {
        Type::String | Type::Number | Type::Boolean => true,
        Type::Union(members) => members.iter().any(|m| holds_primitive(ctx, *m)),
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

// The tuple an expected type asks for: the type itself, or the one tuple member of a union
// (`[string, number] | null`). Two tuple members leave the choice open, so no tuple.
fn tuple_in_context(
    ctx: &CheckContext<'_, '_>,
    expected: TypeId,
) -> Option<Vec<crate::types::TupleElement>> {
    match ctx.arena.get(expected) {
        Type::Tuple(elements) => Some(elements.clone()),
        Type::Union(members) => {
            let mut found = None;
            for &member in members {
                if let Type::Tuple(elements) = ctx.arena.get(member) {
                    if found.is_some() {
                        return None;
                    }
                    found = Some(elements.clone());
                }
            }
            found
        }
        _ => None,
    }
}

// Each written element is typed against the tuple element at its position (the rest
// element's type past the last fixed one), keeping a literal only where that element asks
// for one. The result has the literal's own length, so a literal that is too short or too
// long is reported by the ordinary assignability check against the tuple it was meant for.
fn infer_tuple_literal(
    array: &oxc_ast::ast::ArrayExpression,
    target: &[crate::types::TupleElement],
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> TypeId {
    let rest = target.last().filter(|e| e.rest).map(|e| e.type_id);
    let fixed = target.len() - usize::from(rest.is_some());
    let mut elements = Vec::with_capacity(array.elements.len());
    for (position, element) in array.elements.iter().enumerate() {
        let Some(expr) = element.as_expression() else {
            continue;
        };
        let wanted = if position < fixed {
            Some(target[position].type_id)
        } else {
            rest
        };
        ctx.contextual = wanted;
        let type_id = infer_expression_type(expr, scoping, ctx);
        let type_id = match wanted {
            Some(d) if holds_literal(ctx, d) => type_id,
            _ => crate::types::widen(&ctx.arena, type_id),
        };
        elements.push(crate::types::TupleElement {
            type_id,
            optional: false,
            rest: false,
        });
    }
    ctx.arena.alloc_tuple(elements)
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
    // A tuple type in the context makes the literal a tuple: `const p: [string, number] =
    // ["a", 1]` is the tuple `[string, number]`, not `(string | number)[]`. Without such
    // a context the literal stays an array, as it is in tsc. A literal with a spread or a
    // hole is not read as a tuple: the length would not be known.
    if let Some(target) = expected.and_then(|e| tuple_in_context(ctx, e))
        && array.elements.iter().all(|e| {
            e.as_expression().is_some()
                && !matches!(e, oxc_ast::ast::ArrayExpressionElement::SpreadElement(_))
        })
    {
        return infer_tuple_literal(array, &target, scoping, ctx);
    }

    if ctx.const_context
        && array.elements.iter().all(|e| {
            e.as_expression().is_some()
                && !matches!(e, oxc_ast::ast::ArrayExpressionElement::SpreadElement(_))
        })
    {
        let mut elements = Vec::with_capacity(array.elements.len());
        for element in &array.elements {
            let expr = element.as_expression().expect("checked above");
            ctx.contextual = None;
            let type_id = infer_expression_type(expr, scoping, ctx);
            elements.push(crate::types::TupleElement {
                type_id,
                optional: false,
                rest: false,
            });
        }
        return ctx.arena.alloc_tuple(elements);
    }

    let mut element_types = Vec::with_capacity(array.elements.len());
    // Membership in `element_types`, so a literal of many distinct element types does one set
    // lookup per element instead of a scan of the list (the list keeps first-seen order).
    let mut literal_types: Vec<TypeId> = Vec::new();
    let mut element_seen: std::collections::HashSet<TypeId> =
        std::collections::HashSet::with_capacity(array.elements.len());

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
            if element_seen.insert(item) {
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
        if is_object_literal(expr) && !literal_types.contains(&type_id) {
            literal_types.push(type_id);
        }
        if element_seen.insert(type_id) {
            element_types.push(type_id);
        }
    }

    // `[{ a: 1 }, { b: "x" }]`: the object literals are normalised against each other.
    if literal_types.len() > 1 {
        let normalised = normalize_object_literals(ctx, &literal_types);
        for slot in element_types.iter_mut() {
            if let Some(at) = literal_types.iter().position(|t| t == slot) {
                *slot = normalised[at];
            }
        }
        element_types.dedup();
    }

    let element_type = match element_types.len() {
        0 => ctx.arena.never(),
        1 => element_types[0],
        _ => ctx.arena.alloc_union(element_types),
    };

    ctx.arena.alloc(Type::Array(element_type))
}

// Problem: `cond ? { a: 1 } : { b: "x" }` and `[{ a: 1 }, { b: "x" }]` gave a union whose
// members each lack the other's property, so `z.a` was reported (TS2339), where tsc accepts it.
// Picked: tsc normalises the union of object literals: every member gets the properties it
// lacks from the others as optional `undefined` (`{ a: number; b?: undefined } | { b: string;
// a?: undefined }`). That is what lets `z.a ? ... : z.b` work, and it is what is printed.
// Cost: only literals written in the expression itself are normalised; tsc also does it for a
// variable that holds an object literal, which this does not see (the variable's type does
// not remember it came from one).
pub(super) fn normalize_object_literals(
    ctx: &mut CheckContext<'_, '_>,
    literals: &[TypeId],
) -> Vec<TypeId> {
    // Without strictNullChecks tsc fills the gaps with `any` and `undefined` is assignable
    // to everything; neither is modelled for a missing property, so the union is left alone.
    if !ctx.option(crate::check_options::OptionField::StrictNullChecks) {
        return literals.to_vec();
    }
    let mut shapes: Vec<ObjectType> = Vec::with_capacity(literals.len());
    for &id in literals {
        match ctx.arena.get(id) {
            Type::Object(object)
                if object.string_index.is_none() && object.number_index.is_none() =>
            {
                shapes.push(object.clone())
            }
            _ => return literals.to_vec(),
        }
    }
    let mut names: Vec<std::rc::Rc<str>> = Vec::new();
    for shape in &shapes {
        for property in shape.properties.iter() {
            if !names.contains(&property.name) {
                names.push(property.name.clone());
            }
        }
    }
    let undefined = ctx.arena.undefined();
    let mut out = Vec::with_capacity(literals.len());
    for (&id, shape) in literals.iter().zip(&shapes) {
        let missing: Vec<&std::rc::Rc<str>> = names
            .iter()
            .filter(|name| !shape.properties.iter().any(|p| &p.name == *name))
            .collect();
        if missing.is_empty() {
            out.push(id);
            continue;
        }
        let mut properties: Vec<PropertyEntry> = shape.properties.to_vec();
        for name in missing {
            properties.push(PropertyEntry {
                order: Default::default(),
                name: name.clone(),
                type_id: undefined,
                optional: true,
                is_method: false,
                modifiers: Default::default(),
            });
        }
        out.push(ctx.arena.alloc(Type::Object(ObjectType::new(properties))));
    }
    out
}

/// An expression that is an object literal, seen through parentheses.
pub(super) fn is_object_literal(expr: &oxc_ast::ast::Expression<'_>) -> bool {
    matches!(
        expr.without_parentheses(),
        oxc_ast::ast::Expression::ObjectExpression(_)
    )
}
