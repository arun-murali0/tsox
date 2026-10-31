use oxc_ast::ast::{ArrayPattern, BindingPattern, ObjectPattern, PropertyKey};
use oxc_semantic::Scoping;
use oxc_span::GetSpan;

use crate::arena::TypeId;
use crate::types::{Param, TupleElement, Type};

use super::super::context::CheckContext;
use super::super::expressions::{infer_expression_type, infer_member_access_type};
use super::super::narrow::narrow_to_non_nullish;

pub(crate) fn bind_params(
    params: &oxc_ast::ast::FormalParameters<'_>,
    param_types: &[Param],
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) {
    for (param, declared) in params.items.iter().zip(param_types) {
        bind_pattern(&param.pattern, declared.type_id, scoping, ctx);
    }

    if let Some(rest) = &params.rest {
        if let Some(rest_param) = param_types.last().filter(|p| p.rest) {
            // oxc wraps a rest parameter's own binding one level deeper than an
            // ordinary parameter's, hence rest.rest.argument rather than
            // rest.argument, to make room for the rest parameter's own type
            // annotation alongside its binding pattern.
            bind_pattern(&rest.rest.argument, rest_param.type_id, scoping, ctx);
        }
    }
}

// Recursively binds every identifier a pattern introduces to its corresponding
// type, shared by function parameters (bind_params above) and variable
// declarators (variables.rs), since `function f({x}: T)` and `const {x} = y`
// destructure the same way once a source type is known.
pub(crate) fn bind_pattern(
    pattern: &BindingPattern,
    type_id: TypeId,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) {
    bind_pattern_in(pattern, type_id, false, scoping, ctx);
}

// `from_array_element` is true when `type_id` is the element type of an array this checker
// built from a literal (see bind_array_pattern): its union may then be an artifact of there
// being no tuple type, not something the program wrote.
fn bind_pattern_in(
    pattern: &BindingPattern,
    type_id: TypeId,
    from_array_element: bool,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) {
    match pattern {
        BindingPattern::BindingIdentifier(id) => {
            if let Some(symbol_id) = id.symbol_id.get() {
                ctx.symbols.declare(symbol_id, type_id);
            }
        }
        BindingPattern::ObjectPattern(object) => bind_object_pattern(object, type_id, scoping, ctx),
        BindingPattern::ArrayPattern(array) => {
            bind_array_pattern(array, type_id, from_array_element, scoping, ctx)
        }
        // `{x = 1} = y` or `function f(x = 1)`: the default only ever substitutes
        // for undefined at runtime, so the effective bound type is the source
        // type with undefined (and, as an over-approximation, null too) removed,
        // unioned with the default expression's own widened type.
        BindingPattern::AssignmentPattern(assignment) => {
            let default_type = infer_expression_type(&assignment.right, scoping, ctx);
            let default_type = crate::types::widen(&ctx.arena, default_type);

            let non_nullish = narrow_to_non_nullish(&mut ctx.arena, type_id);
            let effective = ctx.arena.alloc_union(vec![non_nullish, default_type]);
            bind_pattern_in(
                &assignment.left,
                effective,
                from_array_element,
                scoping,
                ctx,
            );
        }
    }
}

fn bind_object_pattern(
    object: &ObjectPattern,
    type_id: TypeId,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) {
    for property in &object.properties {
        let PropertyKey::StaticIdentifier(key) = &property.key else {
            ctx.warning(
                crate::diagnostic_messages::messages::unimplemented_destructuring_key(),
                property.span(),
            );
            continue;
        };
        let property_type = infer_member_access_type(type_id, &key.name, property.span(), ctx);
        bind_pattern(&property.value, property_type, scoping, ctx);
    }

    if let Some(rest) = &object.rest {
        // Unlike array rest below, object rest has no precise type here: the real
        // type would be the source object minus whatever properties were already
        // destructured, which this checker does not compute. Bound to Error
        // rather than left undeclared, so using the rest binding afterward does
        // not cascade into an unrelated "cannot find name" error on top of this
        // warning.
        ctx.warning(
            crate::diagnostic_messages::messages::unimplemented_rest_destructuring(),
            rest.span(),
        );
        bind_pattern(&rest.argument, ctx.arena.error(), scoping, ctx);
    }
}

// Destructuring a tuple. A hole (`const [, b] = t`) still takes up a position. An element
// that is optional, or read from the rest element, may be missing, so it gets `undefined`
// as well. A position past a closed tuple's end is an error unless the binding has a
// default (`const [a, b = 1] = t`), which is what it is for.
fn bind_tuple_pattern(
    array: &ArrayPattern,
    tuple: TypeId,
    elements: &[TupleElement],
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) {
    let rest = elements.last().filter(|e| e.rest).copied();
    let fixed = elements.len() - usize::from(rest.is_some());

    for (position, slot) in array.elements.iter().enumerate() {
        let Some(element_pattern) = slot else {
            continue;
        };
        let bound = if position < fixed {
            let element = elements[position];
            if element.optional {
                ctx.arena
                    .alloc_union(vec![element.type_id, ctx.arena.undefined()])
            } else {
                element.type_id
            }
        } else if let Some(rest) = rest {
            ctx.arena
                .alloc_union(vec![rest.type_id, ctx.arena.undefined()])
        } else {
            if !matches!(element_pattern, BindingPattern::AssignmentPattern(_)) {
                ctx.error(
                    crate::diagnostic_messages::messages::tuple_index_out_of_bounds(
                        &ctx.arena,
                        tuple,
                        elements.len(),
                        position,
                    ),
                    element_pattern.span(),
                );
            }
            ctx.arena.error()
        };
        bind_pattern_in(element_pattern, bound, false, scoping, ctx);
    }

    if let Some(rest_pattern) = &array.rest {
        let remaining: Vec<TupleElement> = elements
            .iter()
            .skip(array.elements.len())
            .copied()
            .collect();
        let rest_type = ctx.arena.alloc_tuple(remaining);
        bind_pattern(&rest_pattern.argument, rest_type, scoping, ctx);
    }
}

fn bind_array_pattern(
    array: &ArrayPattern,
    type_id: TypeId,
    from_array_element: bool,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) {
    // A tuple is read by position: `const [a, b] = pair` gives `a` the first element and
    // `b` the second, and `...rest` the tuple of what is left.
    if let Type::Tuple(elements) = ctx.arena.get(type_id).clone() {
        bind_tuple_pattern(array, type_id, &elements, scoping, ctx);
        return;
    }

    // Whether `type_id` was an honest `T[]`, distinct from the Any/Error/
    // not-an-array fallbacks below, all of which also set `element_type` but
    // must not get the "| undefined" treatment applied further down: adding
    // undefined to an already-Error element type produces a second, cascading
    // diagnostic (e.g. "undefined | error does not match declared return type
    // number") on top of the real array_destructuring_requires_array error
    // already reported for the bad source, the same cascade
    // bind_object_pattern's rest-destructuring case avoids by binding
    // straight to Error instead of wrapping it further.
    let (element_type, is_real_array) = match ctx.arena.get(type_id) {
        Type::Array(element) => (*element, true),
        Type::Any | Type::Error => (type_id, false),
        // Problem: `const [, a] = x || []` with `x: any` reported TS2488 on `never[] | any`.
        // tsc folds any union that has an `any` member into `any`, and destructuring it is
        // fine; this checker keeps the members apart.
        // Picked: read such a union as `any` here, rather than change how unions are built
        // for every other use.
        Type::Union(members)
            if members
                .iter()
                .any(|member| matches!(ctx.arena.get(*member), Type::Any)) =>
        {
            (ctx.arena.any(), false)
        }
        // Problem: `var [a, [b]] = [1, ["x"]]` reported TS2488 on the inner pattern. With no
        // tuple type, the literal is `(number | string[])[]`, so the inner pattern's source
        // is a union with `number` in it; tsc has `[number, string[]]` and sees the array.
        // Picked: for an element of such an array, take the array members of the union and
        // drop the rest as the literal's own widening. A union that has no array member is
        // still reported, and so is any source that is not an element of an array.
        Type::Union(members) if from_array_element => {
            let elements: Vec<TypeId> = members
                .iter()
                .filter_map(|member| match ctx.arena.get(*member) {
                    Type::Array(element) => Some(*element),
                    _ => None,
                })
                .collect();
            if elements.is_empty() {
                ctx.error(
                    crate::diagnostic_messages::messages::array_destructuring_requires_array(
                        &ctx.arena, type_id,
                    ),
                    array.span(),
                );
                (ctx.arena.error(), false)
            } else {
                (ctx.arena.alloc_union(elements), true)
            }
        }
        _ => {
            ctx.error(
                crate::diagnostic_messages::messages::array_destructuring_requires_array(
                    &ctx.arena, type_id,
                ),
                array.span(),
            );
            (ctx.arena.error(), false)
        }
    };

    for element_pattern in array.elements.iter().flatten() {
        // Same reasoning as computed array access (see
        // infer_computed_member_access_type): destructuring past the end of a
        // real array yields undefined at runtime, and this checker cannot
        // prove the array is long enough to cover every position destructured,
        // so each bound element includes undefined regardless of its position,
        // but only when there is a real array element type to union it
        // with in the first place. Any/Error stay as-is, so a bad source
        // (or an already-Any/Error one) doesn't cascade into a second,
        // unrelated diagnostic on every use of the destructured bindings.
        let bound_type = if is_real_array {
            ctx.arena
                .alloc_union(vec![element_type, ctx.arena.undefined()])
        } else {
            element_type
        };
        bind_pattern_in(element_pattern, bound_type, is_real_array, scoping, ctx);
    }

    // Array rest does have a precise type, unlike object rest above: whatever is
    // left over from destructuring an array is still an array of the same
    // element type, regardless of how many elements were taken from the front.
    if let Some(rest) = &array.rest {
        let rest_type = ctx.arena.alloc(Type::Array(element_type));
        bind_pattern(&rest.argument, rest_type, scoping, ctx);
    }
}
