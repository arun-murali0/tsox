//! Variadic tuple reduction (`[...A, ...B]`).
use super::*;

/// `[...A, ...B]` with every operand known: the plain tuple. An operand still generic, or a
/// shape a tuple cannot hold (a rest element that is not last), keeps the variadic node.
pub fn reduce_variadic(arena: &mut TypeArena, elements: Vec<crate::types::TupleElement>) -> TypeId {
    let mut flat: Vec<crate::types::TupleElement> = Vec::with_capacity(elements.len());
    // An operand that is still generic stays a spread in place, while the ones that are known
    // are spliced in around it: `[...[string], ...B]` is `[string, ...B]`.
    let mut unresolved = false;
    for element in &elements {
        if !element.rest {
            flat.push(*element);
            continue;
        }
        // A reference still pointing at its own declaration has no body to read yet.
        if matches!(arena.raw(element.type_id), Type::Ref(_))
            && arena.declared_body(element.type_id).is_none()
        {
            flat.push(*element);
            unresolved = true;
            continue;
        }
        match arena.get(element.type_id).clone() {
            Type::Tuple(inner) => flat.extend(inner),
            Type::Array(item) => flat.push(crate::types::TupleElement {
                type_id: item,
                optional: false,
                rest: true,
            }),
            // An operand that inference could not fix reads as its `unknown[]` constraint.
            Type::Any | Type::Never | Type::Unknown => flat.push(crate::types::TupleElement {
                type_id: if matches!(arena.get(element.type_id), Type::Unknown) {
                    arena.any()
                } else {
                    element.type_id
                },
                optional: false,
                rest: true,
            }),
            _ => {
                flat.push(*element);
                unresolved = true;
            }
        }
    }
    if unresolved {
        return arena.alloc(Type::Variadic(flat));
    }
    let rest_count = flat.iter().filter(|e| e.rest).count();
    let mut seen_optional = false;
    let mut valid = rest_count <= 1 && (rest_count == 0 || flat.last().is_some_and(|e| e.rest));
    for element in &flat {
        if element.optional {
            seen_optional = true;
        } else if seen_optional && !element.rest {
            valid = false;
        }
    }
    if !valid {
        return arena.alloc(Type::Variadic(flat));
    }
    arena.alloc_tuple(flat)
}

/// Whether a parameter type holds a variadic tuple with more than one spread
/// (`(...a: [...A, ...B]) => R`), which cannot be split by position alone.
pub fn has_multi_spread_variadic(arena: &TypeArena, type_id: TypeId) -> bool {
    match arena.get(type_id) {
        Type::Variadic(elements) => elements.iter().filter(|e| e.rest).count() > 1,
        Type::Function(function) => function
            .params
            .iter()
            .any(|p| has_multi_spread_variadic(arena, p.type_id)),
        _ => false,
    }
}
