//! Inference: collecting candidates for type parameters from arguments, conditional `infer`
//! positions and index signatures, and resolving them to bindings.
use super::*;

// Structural inference: walks a generic function's declared parameter type
// alongside a call's actual argument type in lockstep, recording a binding the
// first time it reaches a GenericParameter leaf. This is only the argument-driven
// part: an explicit call-site list like identity<string>(x) is applied by the
// caller (expressions/calls/), which pre-binds those parameters and passes them in as
// `locked` so nothing here can override them.
//
// A second argument resolving to an already-bound parameter does not start a
// union (real TypeScript does not do this either: pair(1, "x") for
// pair<T>(a: T, b: T) is a genuine type error in TypeScript, not T = number |
// string). Instead the two candidates are resolved through ordinary subtyping:
// if one is already a supertype of the other, the binding widens to cover both,
// matching pick(new Dog(), new Animal()) inferring T = Animal. Two candidates
// with no subtype relationship in either direction are left as the first
// binding; the real mismatch is still caught afterward by the normal
// per-argument assignability check in expressions/calls/, the same way it always was.
pub fn infer_type_param_bindings(
    arena: &mut TypeArena,
    param_type: TypeId,
    arg_type: TypeId,
    bindings: &mut Vec<(crate::types::TypeParameterId, TypeId)>,
    locked: &[crate::types::TypeParameterId],
    cache: &mut RelationCache,
) {
    infer_type_param_bindings_inner(
        arena,
        param_type,
        arg_type,
        bindings,
        locked,
        cache,
        &mut SeenPairs::default(),
        false,
    )
}

// Conditional-type `infer` preserves literal candidates. Unlike ordinary generic
// call inference, conditional inference also tracks variance: candidates collected
// from covariant positions combine as a union, while candidates collected only from
// contravariant positions combine as an intersection. TypeScript gives contra
// candidates priority when a variable appears in both directions.
pub(super) fn infer_conditional_type_params(
    arena: &mut TypeArena,
    pattern: TypeId,
    actual: TypeId,
    infer_params: &[TypeParameterId],
    bindings: &mut Vec<(TypeParameterId, TypeId)>,
    _cache: &mut RelationCache,
) {
    let locked: Vec<TypeParameterId> = free_generic_params(arena, pattern)
        .iter()
        .map(|(id, _)| *id)
        .filter(|id| !infer_params.contains(id))
        .collect();
    let mut candidates: Vec<(TypeParameterId, Vec<TypeId>, Vec<TypeId>)> = Vec::new();
    let mut seen = HashSet::default();
    collect_conditional_infer_candidates(
        arena,
        pattern,
        actual,
        infer_params,
        &locked,
        InferenceVariance::Covariant,
        &mut candidates,
        &mut seen,
    );

    for (id, covariant, contravariant) in candidates {
        // Covariant evidence wins when both directions contribute candidates.
        // If there is no covariant evidence, contravariant candidates are
        // intersected, matching TypeScript's conditional-infer rule.
        let chosen = if !covariant.is_empty() {
            if covariant.len() == 1 {
                covariant[0]
            } else {
                arena.alloc_union(covariant)
            }
        } else if contravariant.len() == 1 {
            contravariant[0]
        } else {
            arena
                .alloc_intersection(contravariant)
                .unwrap_or_else(|_| arena.error())
        };
        if let Some((_, existing)) = bindings.iter_mut().find(|(bound, _)| *bound == id) {
            *existing = chosen;
        } else {
            bindings.push((id, chosen));
        }
    }
}

/// Whether two application nodes are of one generic declaration. Each instantiation names a
/// slot of its own, so `P<number>` and `P<infer U>` differ in slot and agree in the declaration
/// they were made from, which is what lets their arguments be matched pairwise.
pub(super) fn same_application(
    arena: &TypeArena,
    left: TypeId,
    left_slot: crate::arena::DeclSlot,
    right: TypeId,
    right_slot: crate::arena::DeclSlot,
) -> bool {
    left_slot == right_slot
        || matches!(
            (arena.app_base_of(left), arena.app_base_of(right)),
            (Some(a), Some(b)) if a == b
        )
}

pub(super) fn collect_conditional_infer_candidates(
    arena: &mut TypeArena,
    pattern: TypeId,
    actual: TypeId,
    infer_params: &[TypeParameterId],
    locked: &[TypeParameterId],
    variance: InferenceVariance,
    out: &mut Vec<(TypeParameterId, Vec<TypeId>, Vec<TypeId>)>,
    seen: &mut HashSet<(TypeId, TypeId, InferenceVariance), FxBuildHasher>,
) {
    if !seen.insert((pattern, actual, variance)) {
        return;
    }

    // Applications retain their declaration arguments even when the instantiated
    // body is opaque (Promise<T>, Record<K, V>, and user generic aliases).
    if let (Type::App(pattern_slot, pattern_args, _), Type::App(actual_slot, actual_args, _)) =
        (arena.raw(pattern).clone(), arena.raw(actual).clone())
        && same_application(arena, pattern, pattern_slot, actual, actual_slot)
        && pattern_args.len() == actual_args.len()
    {
        for (&p, &a) in pattern_args.iter().zip(&actual_args) {
            collect_conditional_infer_candidates(
                arena,
                p,
                a,
                infer_params,
                locked,
                variance,
                out,
                seen,
            );
        }
        return;
    }

    // Inferring from a union takes each member in turn (tsc infers from every
    // constituent of a union source), which is how `(A | B) extends (k: infer I) => void`
    // sees both functions. A pattern that is itself an infer parameter takes the union
    // whole, and a pattern union is matched below as one.
    if let Type::Union(members) = arena.get(actual).clone()
        && !matches!(
            arena.get(pattern),
            Type::Union(_) | Type::GenericParameter(..)
        )
    {
        for member in members {
            collect_conditional_infer_candidates(
                arena,
                pattern,
                member,
                infer_params,
                locked,
                variance,
                out,
                seen,
            );
        }
        return;
    }

    if let Type::Template(template) = arena.get(pattern).clone()
        && let Type::StringLiteral(text) = arena.get(actual).clone()
    {
        if let Some(captures) = match_template(arena, &template, &text) {
            for (&part, capture) in template.types.iter().zip(captures) {
                if let Type::GenericParameter(id, _, _) = arena.get(part).clone()
                    && infer_params.contains(&id)
                    && !locked.contains(&id)
                {
                    let literal = arena.alloc(Type::StringLiteral(capture));
                    match out
                        .iter_mut()
                        .find(|(candidate_id, _, _)| *candidate_id == id)
                    {
                        Some((_, covariant, _)) => covariant.push(literal),
                        None => out.push((id, vec![literal], Vec::new())),
                    }
                }
            }
        }
        return;
    }

    match arena.get(pattern).clone() {
        Type::GenericParameter(id, _, _) if infer_params.contains(&id) && !locked.contains(&id) => {
            let candidate = out
                .iter_mut()
                .find(|(candidate_id, _, _)| *candidate_id == id);
            if let Some((_, covariant, contravariant)) = candidate {
                match variance {
                    InferenceVariance::Covariant => covariant.push(actual),
                    InferenceVariance::Contravariant => contravariant.push(actual),
                    InferenceVariance::Invariant => {
                        covariant.push(actual);
                        contravariant.push(actual);
                    }
                }
            } else {
                let (mut covariant, mut contravariant) = (Vec::new(), Vec::new());
                match variance {
                    InferenceVariance::Covariant => covariant.push(actual),
                    InferenceVariance::Contravariant => contravariant.push(actual),
                    InferenceVariance::Invariant => {
                        covariant.push(actual);
                        contravariant.push(actual);
                    }
                }
                out.push((id, covariant, contravariant));
            }
        }
        Type::Array(pattern_element) => {
            if let Type::Array(actual_element) = arena.get(actual) {
                let actual_element = *actual_element;
                collect_conditional_infer_candidates(
                    arena,
                    pattern_element,
                    actual_element,
                    infer_params,
                    locked,
                    variance,
                    out,
                    seen,
                );
            }
        }
        Type::Variadic(pattern_elements) => {
            // `[unknown, ...infer R]`: leading elements by position, then the spread takes
            // what is left of the actual tuple (less any elements fixed after it).
            if let Type::Tuple(actual_elements) = arena.get(actual).clone() {
                let spread_at = pattern_elements.iter().position(|e| e.rest);
                let fixed_before = spread_at.unwrap_or(pattern_elements.len());
                for (index, pattern_element) in
                    pattern_elements.iter().take(fixed_before).enumerate()
                {
                    let Some(actual_element) = actual_elements.get(index) else {
                        break;
                    };
                    if actual_element.rest {
                        break;
                    }
                    collect_conditional_infer_candidates(
                        arena,
                        pattern_element.type_id,
                        actual_element.type_id,
                        infer_params,
                        locked,
                        variance,
                        out,
                        seen,
                    );
                }
                if let Some(at) = spread_at {
                    let after = pattern_elements.len() - at - 1;
                    if actual_elements.len() >= at + after {
                        let middle: Vec<crate::types::TupleElement> =
                            actual_elements[at..actual_elements.len() - after].to_vec();
                        let operand = pattern_elements[at].type_id;
                        let middle_type = arena.alloc_tuple(middle);
                        collect_conditional_infer_candidates(
                            arena,
                            operand,
                            middle_type,
                            infer_params,
                            locked,
                            variance,
                            out,
                            seen,
                        );
                        for (offset, pattern_element) in
                            pattern_elements[at + 1..].iter().enumerate()
                        {
                            let actual_element =
                                &actual_elements[actual_elements.len() - after + offset];
                            collect_conditional_infer_candidates(
                                arena,
                                pattern_element.type_id,
                                actual_element.type_id,
                                infer_params,
                                locked,
                                variance,
                                out,
                                seen,
                            );
                        }
                    }
                }
            }
        }
        Type::Tuple(pattern_elements) => {
            // `[infer H, ...]`: the elements before a rest element are matched by
            // position. A rest element, and anything after it, stands for a variable
            // number of elements and gives nothing to match by position.
            if let Type::Tuple(actual_elements) = arena.get(actual).clone() {
                for (index, pattern_element) in pattern_elements.iter().enumerate() {
                    if pattern_element.rest {
                        break;
                    }
                    let Some(actual_element) = actual_elements.get(index) else {
                        break;
                    };
                    if actual_element.rest {
                        break;
                    }
                    collect_conditional_infer_candidates(
                        arena,
                        pattern_element.type_id,
                        actual_element.type_id,
                        infer_params,
                        locked,
                        variance,
                        out,
                        seen,
                    );
                }
            }
        }
        Type::Object(pattern_object) => {
            if let Type::Object(actual_object) = arena.get(actual).clone() {
                for property in pattern_object.properties.iter() {
                    if let Some(actual_property) = actual_object
                        .properties
                        .iter()
                        .find(|candidate| candidate.name == property.name)
                    {
                        collect_conditional_infer_candidates(
                            arena,
                            property.type_id,
                            actual_property.type_id,
                            infer_params,
                            locked,
                            variance,
                            out,
                            seen,
                        );
                    }
                }
            }
        }
        Type::Function(pattern_function) => {
            if let Type::Function(actual_function) = arena.get(actual).clone() {
                let mut nested_locked = locked.to_vec();
                for parameter in &pattern_function.type_params {
                    if !nested_locked.contains(&parameter.id) {
                        nested_locked.push(parameter.id);
                    }
                }
                // `(...args: infer P)`: the rest parameter stands for every parameter of the
                // other signature from its position on, so P is the tuple of those.
                let tuple_rest = rest_parameter_standing_for_tuple(arena, &pattern_function.params);
                if let Some(rest) = tuple_rest {
                    let rest_type = pattern_function.params[rest].type_id;
                    let remaining =
                        remaining_parameters_as_tuple(arena, &actual_function.params, rest);
                    collect_conditional_infer_candidates(
                        arena,
                        rest_type,
                        remaining,
                        infer_params,
                        &nested_locked,
                        variance.reversed(),
                        out,
                        seen,
                    );
                }
                let count = tuple_rest.unwrap_or(
                    pattern_function
                        .params
                        .len()
                        .max(actual_function.params.len()),
                );
                for position in 0..count {
                    if let (Some(pattern_param), Some(actual_param)) = (
                        crate::subtyping::param_type_at(arena, &pattern_function.params, position),
                        crate::subtyping::param_type_at(arena, &actual_function.params, position),
                    ) {
                        collect_conditional_infer_candidates(
                            arena,
                            pattern_param,
                            actual_param,
                            infer_params,
                            &nested_locked,
                            variance.reversed(),
                            out,
                            seen,
                        );
                    }
                }
                collect_conditional_infer_candidates(
                    arena,
                    pattern_function.return_type,
                    actual_function.return_type,
                    infer_params,
                    &nested_locked,
                    variance,
                    out,
                    seen,
                );
            }
        }
        Type::Intersection(pattern_members) => {
            for member in pattern_members {
                if contains_type_param(arena, member) {
                    collect_conditional_infer_candidates(
                        arena,
                        member,
                        actual,
                        infer_params,
                        locked,
                        variance,
                        out,
                        seen,
                    );
                }
            }
        }
        _ => {}
    }
}

// Inference mutates the arena (widening and union building allocate), so it cannot
// keep one SemanticQueries alive across its steps: that holds a shared borrow of the
// arena for its whole life. Building one per question keeps the borrow to the call.
// The reason to go through it at all is that candidates are compared against the same
// existing binding over and over (`allSame(1, 2, ..., 8)`), and those repeats were the
// one place the cache never saw.
pub fn is_subtype_via_cache(
    arena: &TypeArena,
    cache: &mut RelationCache,
    sub: TypeId,
    sup: TypeId,
) -> bool {
    SemanticQueries::new(arena, cache).is_subtype(sub, sup)
}

// Whether a bound asks for narrow types: it is, or contains, a primitive or a literal.
pub(super) fn bound_keeps_literals(arena: &TypeArena, bound: TypeId) -> bool {
    let is_primitive_or_literal = |id: TypeId| {
        matches!(
            arena.get(id),
            Type::String
                | Type::Number
                | Type::Boolean
                | Type::StringLiteral(_)
                | Type::NumberLiteral(_)
                | Type::BooleanLiteral(_)
        )
    };
    match arena.get(bound) {
        Type::Union(members) => members
            .iter()
            .any(|&member| is_primitive_or_literal(member)),
        // `K extends keyof T`: a key is a string literal, so the argument keeps its
        // literal type, as it does under a bound that is itself a literal union.
        Type::Keyof(_) => true,
        _ => is_primitive_or_literal(bound),
    }
}

// Which primitive a literal, or a union made only of literals, belongs to. None for
// anything else, and for a union that mixes primitives.
pub(super) fn literal_base(arena: &TypeArena, id: TypeId) -> Option<u8> {
    let base_of = |id: TypeId| match arena.get(id) {
        Type::StringLiteral(_) => Some(0),
        Type::NumberLiteral(_) => Some(1),
        Type::BooleanLiteral(_) => Some(2),
        _ => None,
    };
    match arena.get(id) {
        Type::Union(members) => {
            let mut bases = members.iter().map(|&member| base_of(member));
            let first = bases.next()??;
            bases.all(|base| base == Some(first)).then_some(first)
        }
        _ => base_of(id),
    }
}

pub(super) fn same_literal_base(arena: &TypeArena, a: TypeId, b: TypeId) -> bool {
    matches!((literal_base(arena, a), literal_base(arena, b)), (Some(x), Some(y)) if x == y)
}

/// Merge one already-collected inference candidate into a type parameter's chosen
/// binding. Kept in one place so the structural collector and the new candidate
/// finalizer use identical subtype/literal-widening policy.
pub fn merge_inference_candidate(
    arena: &mut TypeArena,
    id: crate::types::TypeParameterId,
    candidate: TypeId,
    keeps_literals: bool,
    bindings: &mut Vec<(crate::types::TypeParameterId, TypeId)>,
    cache: &mut RelationCache,
) {
    merge_inference_candidate_with_constraint(
        arena,
        id,
        candidate,
        keeps_literals,
        None,
        bindings,
        cache,
    );
}

/// Merge covariant evidence. The constraint is accepted so callers keep passing it, but it
/// is not used to widen unrelated candidates: tsc keeps the leftmost one.
pub fn merge_inference_candidate_with_constraint(
    arena: &mut TypeArena,
    id: crate::types::TypeParameterId,
    candidate: TypeId,
    keeps_literals: bool,
    _constraint: Option<TypeId>,
    bindings: &mut Vec<(crate::types::TypeParameterId, TypeId)>,
    cache: &mut RelationCache,
) {
    let Some(index) = bindings.iter().position(|(bound, _)| *bound == id) else {
        bindings.push((id, candidate));
        return;
    };

    let existing = bindings[index].1;
    if is_subtype_via_cache(arena, cache, candidate, existing) {
        // The existing binding already covers this candidate.
    } else if is_subtype_via_cache(arena, cache, existing, candidate) {
        bindings[index].1 = candidate;
    } else if keeps_literals && same_literal_base(arena, existing, candidate) {
        bindings[index].1 = arena.alloc_union(vec![existing, candidate]);
    }
    // Otherwise the candidates are unrelated and the first one stays, which is what tsc
    // does (getCommonSupertype takes the leftmost candidate when none is a supertype of
    // the rest). The later argument is then reported by the assignability pass, so
    // `choose(giraffe, elephant)` is TS2345 and is not widened to the shared constraint.
}

// The (param_type, arg_type) pairs one inference call has already matched, or is
// matching right now.
pub(super) type SeenPairs = HashSet<(TypeId, TypeId), FxBuildHasher>;

// A recursive param_type (e.g. `Box<T>`'s own self-referential `next: Box<T>`,
// still holding the bare GenericParameter shape while it's being matched
// against a real argument) can lead back to matching the same (param_type,
// arg_type) pair against itself before the first match finishes. `seen` holds the
// pairs this call has started on; reaching one again contributes nothing further
// worth inferring, so it's simply skipped rather than walked again.
//
// Problem: `seen` used to be a stack, and a pair was removed again as soon as its
// match finished. That only stops a pair from containing itself; the same pair
// reached by a different route was matched in full again. With classes that point
// at each other (`class C0 { p1: C1; p2: C2; ... }`, as in TypeScript's
// resolvingClassDeclarationWhenInBaseTypeResolution) the number of routes grows
// exponentially, and a call such as `use(c0)` for a parameter of type C0 never
// finished, even though nothing in it is generic.
// Picked: keep every pair for the rest of the call. A pair matched a second time
// would offer the same candidate to the same parameters, and offering a candidate
// the binding already covers changes nothing, so the only thing skipped is repeated
// work. tsc keeps its own visited pairs for the whole inference in the same way.
// Cost: if a binding is widened after a pair was matched, that pair is not offered
// again against the wider binding. Both the first match and a repeat are compared
// with the binding as it stands, so a candidate that was ignored once for being
// unrelated to the binding can no longer be picked up by a later route to it.
/// The index of a rest parameter whose type is not itself an array or tuple, such as the
/// `ARGS` of `(...args: ARGS) => R`. That parameter stands for every remaining parameter
/// of the other signature, so it is inferred as a tuple rather than matched per position.
pub(super) fn rest_parameter_standing_for_tuple(
    arena: &TypeArena,
    params: &[crate::types::Param],
) -> Option<usize> {
    params
        .iter()
        .position(|p| p.rest && !matches!(arena.get(p.type_id), Type::Array(_) | Type::Tuple(_)))
}

/// The parameters of a signature from `index` on, as a tuple type. A rest parameter
/// contributes its element type and stays a rest element.
pub(super) fn remaining_parameters_as_tuple(
    arena: &mut TypeArena,
    params: &[crate::types::Param],
    index: usize,
) -> TypeId {
    let elements: Vec<crate::types::TupleElement> = params
        .iter()
        .skip(index)
        .map(|a| crate::types::TupleElement {
            type_id: match (a.rest, arena.get(a.type_id)) {
                (true, Type::Array(element)) => *element,
                _ => a.type_id,
            },
            optional: a.optional,
            rest: a.rest,
        })
        .collect();
    arena.alloc_tuple(elements)
}

pub(super) fn infer_type_param_bindings_inner(
    arena: &mut TypeArena,
    param_type: TypeId,
    arg_type: TypeId,
    bindings: &mut Vec<(crate::types::TypeParameterId, TypeId)>,
    locked: &[crate::types::TypeParameterId],
    cache: &mut RelationCache,
    seen: &mut SeenPairs,
    preserve_literals: bool,
) {
    if !seen.insert((param_type, arg_type)) {
        return;
    }
    infer_type_param_bindings_uncached(
        arena,
        param_type,
        arg_type,
        bindings,
        locked,
        cache,
        seen,
        preserve_literals,
    );
}

pub(super) fn infer_type_param_bindings_uncached(
    arena: &mut TypeArena,
    param_type: TypeId,
    arg_type: TypeId,
    bindings: &mut Vec<(crate::types::TypeParameterId, TypeId)>,
    locked: &[crate::types::TypeParameterId],
    cache: &mut RelationCache,
    seen: &mut SeenPairs,
    preserve_literals: bool,
) {
    // TypeArena::get intentionally exposes an application's instantiated body, but
    // conditional inference needs the type arguments themselves for opaque built-ins
    // such as Promise<T>, whose body does not mention T. Match same-declaration
    // applications before falling back to their structural bodies.
    if let (Type::App(param_slot, param_args, _), Type::App(arg_slot, arg_args, _)) =
        (arena.raw(param_type).clone(), arena.raw(arg_type).clone())
        && same_application(arena, param_type, param_slot, arg_type, arg_slot)
        && param_args.len() == arg_args.len()
    {
        for (&parameter, &argument) in param_args.iter().zip(&arg_args) {
            infer_type_param_bindings_inner(
                arena,
                parameter,
                argument,
                bindings,
                locked,
                cache,
                seen,
                preserve_literals,
            );
        }
        return;
    }

    // The same, found through any alias wrapped around a built-in application.
    // `Promise<T>` and `Record<K, V>` read as an empty body through `get`, so their
    // arguments are matched off the applications themselves.
    if let Some((param_slot, param_args)) = arena.builtin_app_args(param_type)
        && let Some((arg_slot, arg_args)) = arena.builtin_app_args(arg_type)
    {
        if param_slot == arg_slot && param_args.len() == arg_args.len() {
            for (param_arg, arg_arg) in param_args.into_iter().zip(arg_args) {
                infer_type_param_bindings_inner(
                    arena,
                    param_arg,
                    arg_arg,
                    bindings,
                    locked,
                    cache,
                    seen,
                    preserve_literals,
                );
            }
        }
        return;
    }
    match arena.get(param_type).clone() {
        Type::GenericParameter(id, _, bound) => {
            // A parameter bound by an explicit call-site type argument
            // (identity<string>(x)) is not up for renegotiation by whatever
            // argument happens to line up with it structurally; explicit
            // wins outright, the same way TypeScript itself treats an
            // explicit type argument as authoritative rather than a hint.
            if locked.contains(&id) {
                return;
            }

            // A literal argument is widened, so identity(5) infers number and not
            // the literal 5, which is what a bare generic call is expected to give.
            // Unless the parameter's bound mentions a primitive or literal type
            // (`T extends "a" | "b"`, `T extends number`): the bound is then asking
            // for the narrow type, widening would break it (`pick("a")` would bind T
            // to string, which does not satisfy `"a" | "b"`), and TypeScript keeps the
            // literal in exactly this case.
            let keeps_literals =
                preserve_literals || bound.is_some_and(|bound| bound_keeps_literals(arena, bound));
            let candidate = if keeps_literals {
                arg_type
            } else {
                crate::types::widen(arena, arg_type)
            };

            // Conditional infer candidates retain their literal shape. When the same
            // infer variable occurs more than once, combine candidate evidence instead
            // of dropping a candidate from a different primitive family.
            if preserve_literals {
                if let Some((_, existing)) = bindings.iter_mut().find(|(bound, _)| *bound == id) {
                    *existing = arena.alloc_union(vec![*existing, candidate]);
                } else {
                    bindings.push((id, candidate));
                }
            } else {
                // `pair(1, 2)` for `T extends number` keeps both literal candidates as a
                // union; unconstrained candidates were widened above before they arrive here.
                merge_inference_candidate(arena, id, candidate, keeps_literals, bindings, cache);
            }
        }
        Type::Array(param_element) => {
            // A tuple argument is an array of what its elements are: every element is a
            // candidate for the array's element type.
            if let Type::Tuple(arg_elements) = arena.get(arg_type).clone() {
                // One candidate, the union of the elements: `[string, number]` against
                // `T[]` gives `string | number`, not two candidates that fight over T.
                let mut members: Vec<TypeId> = Vec::with_capacity(arg_elements.len());
                for element in &arg_elements {
                    if !members.contains(&element.type_id) {
                        members.push(element.type_id);
                    }
                }
                let element = match members.len() {
                    0 => arena.never(),
                    1 => members[0],
                    _ => arena.alloc_union(members),
                };
                infer_type_param_bindings_inner(
                    arena,
                    param_element,
                    element,
                    bindings,
                    locked,
                    cache,
                    seen,
                    preserve_literals,
                );
                return;
            }
            let arg_element = match arena.get(arg_type) {
                Type::Array(element) => *element,
                _ => return,
            };
            infer_type_param_bindings_inner(
                arena,
                param_element,
                arg_element,
                bindings,
                locked,
                cache,
                seen,
                preserve_literals,
            );
        }
        // `[...T, number]` against `[string, number]`: the fixed elements around the spread
        // match by position and T takes what lies between them.
        Type::Variadic(param_elements) => {
            let Type::Tuple(arg_elements) = arena.get(arg_type).clone() else {
                return;
            };
            let Some((pairs, spread)) = variadic_split(arena, &param_elements, &arg_elements)
            else {
                return;
            };
            for (wanted, actual) in pairs.into_iter().chain(std::iter::once(spread)) {
                infer_type_param_bindings_inner(
                    arena,
                    wanted,
                    actual,
                    bindings,
                    locked,
                    cache,
                    seen,
                    preserve_literals,
                );
            }
        }
        // `[T, U]` against `[string, number]`: element by element, a rest element taking
        // every argument element past the fixed ones. Against an array it matches nothing,
        // since an array promises no particular element at any position.
        Type::Tuple(param_elements) => {
            let Type::Tuple(arg_elements) = arena.get(arg_type).clone() else {
                return;
            };
            let rest = param_elements.last().filter(|e| e.rest).copied();
            let fixed = param_elements.len() - usize::from(rest.is_some());
            for (position, arg_element) in arg_elements.iter().enumerate() {
                let wanted = if position < fixed {
                    param_elements[position].type_id
                } else if let Some(rest) = rest {
                    rest.type_id
                } else {
                    break;
                };
                // An argument rest element (`...number[]`) is the element type itself.
                infer_type_param_bindings_inner(
                    arena,
                    wanted,
                    arg_element.type_id,
                    bindings,
                    locked,
                    cache,
                    seen,
                    preserve_literals,
                );
            }
        }
        Type::Object(param_object) => {
            let Type::Object(arg_object) = arena.get(arg_type).clone() else {
                return;
            };
            for param_prop in param_object.properties.iter() {
                if let Some(arg_prop) = arg_object
                    .properties
                    .iter()
                    .find(|p| p.name == param_prop.name)
                {
                    infer_type_param_bindings_inner(
                        arena,
                        param_prop.type_id,
                        arg_prop.type_id,
                        bindings,
                        locked,
                        cache,
                        seen,
                        preserve_literals,
                    );
                }
            }
            for (signature, from) in index_inference_sources(&param_object, &arg_object) {
                infer_type_param_bindings_inner(
                    arena,
                    signature,
                    from,
                    bindings,
                    locked,
                    cache,
                    seen,
                    preserve_literals,
                );
            }
        }
        Type::Function(param_fn) => {
            let Type::Function(arg_fn) = arena.get(arg_type).clone() else {
                return;
            };
            // A nested signature's own type parameters are binders, not inference targets
            // of the enclosing call. Free outer parameters in the same signature remain
            // candidates (for example `<U>(x: U) => T` can still infer the outer T).
            let mut nested_locked = locked.to_vec();
            for parameter in &param_fn.type_params {
                if !nested_locked.contains(&parameter.id) {
                    nested_locked.push(parameter.id);
                }
            }
            // Match parameters by call position rather than zipping declarations.
            // Optional and rest parameters can make the declared lists differ while
            // still describing overlapping argument positions. `param_type_at`
            // also unwraps a rest array to its element type for each consumed slot.
            //
            // `(...args: ARGS) => R` against `(a: string, b?: number) => R`: the rest
            // parameter stands for all the remaining parameters, so ARGS is the tuple
            // `[a: string, b?: number]`. Positions before it are matched as usual.
            let tuple_rest = rest_parameter_standing_for_tuple(arena, &param_fn.params);
            let checked_positions = match tuple_rest {
                Some(index) => index,
                None => param_fn.params.len().max(arg_fn.params.len()),
            };
            for position in 0..checked_positions {
                if let (Some(parameter), Some(argument)) = (
                    crate::subtyping::param_type_at(arena, &param_fn.params, position),
                    crate::subtyping::param_type_at(arena, &arg_fn.params, position),
                ) {
                    infer_type_param_bindings_inner(
                        arena,
                        parameter,
                        argument,
                        bindings,
                        &nested_locked,
                        cache,
                        seen,
                        preserve_literals,
                    );
                }
            }
            if let Some(index) = tuple_rest {
                let tuple = remaining_parameters_as_tuple(arena, &arg_fn.params, index);
                infer_type_param_bindings_inner(
                    arena,
                    param_fn.params[index].type_id,
                    tuple,
                    bindings,
                    &nested_locked,
                    cache,
                    seen,
                    preserve_literals,
                );
            }
            infer_type_param_bindings_inner(
                arena,
                param_fn.return_type,
                arg_fn.return_type,
                bindings,
                &nested_locked,
                cache,
                seen,
                preserve_literals,
            );
        }
        // A parameter typed `T & { id: number }`. tsc infers into each member from the
        // argument as a whole, so `T` here is the argument's own type, and a member
        // with no type parameter in it has nothing to say.
        Type::Intersection(param_members) => {
            for member in param_members {
                if contains_type_param(arena, member) {
                    infer_type_param_bindings_inner(
                        arena,
                        member,
                        arg_type,
                        bindings,
                        locked,
                        cache,
                        seen,
                        preserve_literals,
                    );
                }
            }
        }
        // A parameter typed `T | undefined`, `T | null` or the like. TypeScript
        // first sets aside whatever part of the argument a concrete member of the
        // union already accounts for (an `undefined` argument against `T |
        // undefined`), then infers the type parameter from what is left. The
        // leftover members are combined into one candidate, so passing a
        // `number | string` binds T to `number | string` in one step rather than
        // to `number` and then failing on `string`. An `any` or error argument is
        // never set aside, since it is compatible with every concrete member and
        // would otherwise leave T with nothing to infer from.
        Type::Union(param_members) => {
            let arg_members = match arena.get(arg_type) {
                Type::Union(members) => members.clone(),
                _ => vec![arg_type],
            };
            let remaining: Vec<TypeId> = arg_members
                .into_iter()
                .filter(|&member| {
                    if matches!(arena.get(member), Type::Any | Type::Error) {
                        return true;
                    }
                    !param_members.iter().any(|&concrete| {
                        !contains_type_param(arena, concrete)
                            && is_subtype_via_cache(arena, cache, member, concrete)
                    })
                })
                .collect();
            if remaining.is_empty() {
                return;
            }
            let remainder = if remaining.len() == 1 {
                remaining[0]
            } else {
                arena.alloc_union(remaining)
            };
            for &member in &param_members {
                if contains_type_param(arena, member) {
                    infer_type_param_bindings_inner(
                        arena,
                        member,
                        remainder,
                        bindings,
                        locked,
                        cache,
                        seen,
                        preserve_literals,
                    );
                }
            }
        }
        _ => {}
    }
}

/// Direction in which an inference candidate was observed. Function parameters
/// reverse the direction; return types and ordinary structural members preserve it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InferenceVariance {
    Covariant,
    Contravariant,
    Invariant,
}

impl InferenceVariance {
    fn reversed(self) -> Self {
        match self {
            Self::Covariant => Self::Contravariant,
            Self::Contravariant => Self::Covariant,
            Self::Invariant => Self::Invariant,
        }
    }
}

/// One raw inference candidate, before candidates from different argument positions
/// are reconciled. Keeping variance here is important: `T` in a return position and
/// `T` in a callback parameter do not have the same merge rule.
pub type RawInferenceCandidate = (
    crate::types::TypeParameterId,
    TypeId,
    InferenceVariance,
    bool,
    Option<TypeId>,
    // Whether the parameter sits at the top level of the declared parameter type (alone,
    // or as a union/intersection member), and the argument type before any widening.
    // tsc widens literals only when every candidate for a parameter is top-level.
    bool,
    TypeId,
);

pub fn collect_inference_candidates(
    arena: &mut TypeArena,
    param_type: TypeId,
    arg_type: TypeId,
    locked: &[crate::types::TypeParameterId],
    out: &mut Vec<RawInferenceCandidate>,
    cache: &mut RelationCache,
) {
    let mut seen = HashSet::default();
    collect_inference_candidates_inner(
        arena,
        param_type,
        arg_type,
        locked,
        InferenceVariance::Covariant,
        true,
        out,
        cache,
        &mut seen,
    );
}

pub(super) fn collect_inference_candidates_inner(
    arena: &mut TypeArena,
    param_type: TypeId,
    arg_type: TypeId,
    locked: &[crate::types::TypeParameterId],
    variance: InferenceVariance,
    top_level: bool,
    out: &mut Vec<RawInferenceCandidate>,
    cache: &mut RelationCache,
    seen: &mut HashSet<(TypeId, TypeId, InferenceVariance), FxBuildHasher>,
) {
    if !seen.insert((param_type, arg_type, variance)) {
        return;
    }
    // `Promise<T>` and `Record<K, V>` read as an empty body through `get`, so their
    // arguments are matched off the applications themselves.
    if let Some((param_slot, param_args)) = arena.builtin_app_args(param_type)
        && let Some((arg_slot, arg_args)) = arena.builtin_app_args(arg_type)
    {
        if param_slot == arg_slot && param_args.len() == arg_args.len() {
            for (param_arg, arg_arg) in param_args.into_iter().zip(arg_args) {
                if contains_type_param(arena, param_arg) {
                    collect_inference_candidates_inner(
                        arena, param_arg, arg_arg, locked, variance, false, out, cache, seen,
                    );
                }
            }
        }
        return;
    }
    // Two applications of the same generic (`Promise<T>` against `Promise<number>`,
    // `Box<T>` against `Box<string>`) infer through their type arguments, pairwise.
    if let (Type::App(param_slot, param_args, _), Type::App(arg_slot, arg_args, _)) =
        (arena.get(param_type).clone(), arena.get(arg_type).clone())
        && same_application(arena, param_type, param_slot, arg_type, arg_slot)
        && param_args.len() == arg_args.len()
    {
        for (param_arg, arg_arg) in param_args.into_iter().zip(arg_args) {
            if contains_type_param(arena, param_arg) {
                collect_inference_candidates_inner(
                    arena, param_arg, arg_arg, locked, variance, false, out, cache, seen,
                );
            }
        }
        return;
    }
    match arena.get(param_type).clone() {
        Type::GenericParameter(id, _, bound) => {
            if locked.contains(&id) {
                return;
            }
            let preserves_literals = bound.is_some_and(|b| bound_keeps_literals(arena, b));
            // tsc widens only top-level candidates (`T` itself as the parameter type). A
            // candidate found inside another type (`{ kind: T }`) keeps a literal the
            // argument's declared type carries, so a contravariant use of the same
            // parameter elsewhere (`(arg: { kind: T }) => void`) can still agree with it.
            let candidate = if preserves_literals || !top_level {
                arg_type
            } else {
                crate::types::widen(arena, arg_type)
            };
            // Keep the declaration's constraint with the candidate. When two
            // covariant candidates are unrelated (e.g. Giraffe and Elephant),
            // the constraint can provide their common inference root if both
            // candidates satisfy it. Without the constraint, reconciliation
            // intentionally keeps the existing mismatch behavior.
            out.push((
                id,
                candidate,
                variance,
                preserves_literals,
                bound,
                top_level,
                arg_type,
            ));
        }
        Type::Array(element) => {
            // A tuple argument is an array of what its elements are: one candidate, the
            // union of the elements, so `[string, number]` against `T[]` gives
            // `string | number` and not two candidates that fight over T.
            if let Type::Tuple(arg_elements) = arena.get(arg_type).clone() {
                let mut members: Vec<TypeId> = Vec::with_capacity(arg_elements.len());
                for element in &arg_elements {
                    if !members.contains(&element.type_id) {
                        members.push(element.type_id);
                    }
                }
                let joined = match members.len() {
                    0 => arena.never(),
                    1 => members[0],
                    _ => arena.alloc_union(members),
                };
                collect_inference_candidates_inner(
                    arena, element, joined, locked, variance, false, out, cache, seen,
                );
                return;
            }
            let Type::Array(arg_element) = arena.get(arg_type) else {
                return;
            };
            let arg_element = *arg_element;
            collect_inference_candidates_inner(
                arena,
                element,
                arg_element,
                locked,
                variance,
                false,
                out,
                cache,
                seen,
            );
        }
        Type::Variadic(param_elements) => {
            let Type::Tuple(arg_elements) = arena.get(arg_type).clone() else {
                return;
            };
            let Some((pairs, spread)) = variadic_split(arena, &param_elements, &arg_elements)
            else {
                return;
            };
            for (wanted, actual) in pairs.into_iter().chain(std::iter::once(spread)) {
                collect_inference_candidates_inner(
                    arena, wanted, actual, locked, variance, false, out, cache, seen,
                );
            }
        }
        // `[T, U]` against `[string, number]`: element by element, a rest element taking
        // every argument element past the fixed ones. Against an array it matches nothing,
        // since an array promises no particular element at any position.
        Type::Tuple(param_elements) => {
            let Type::Tuple(arg_elements) = arena.get(arg_type).clone() else {
                return;
            };
            let rest = param_elements.last().filter(|e| e.rest).copied();
            let fixed = param_elements.len() - usize::from(rest.is_some());
            for (position, arg_element) in arg_elements.iter().enumerate() {
                let wanted = if position < fixed {
                    param_elements[position].type_id
                } else if let Some(rest) = rest {
                    rest.type_id
                } else {
                    break;
                };
                collect_inference_candidates_inner(
                    arena,
                    wanted,
                    arg_element.type_id,
                    locked,
                    variance,
                    false,
                    out,
                    cache,
                    seen,
                );
            }
        }
        Type::Object(param_object) => {
            let Type::Object(arg_object) = arena.get(arg_type).clone() else {
                return;
            };
            for param_prop in param_object.properties.iter() {
                if let Some(arg_prop) = arg_object
                    .properties
                    .iter()
                    .find(|p| p.name == param_prop.name)
                {
                    // A method member is still a function type for inference. Its
                    // parameter positions reverse variance and its return position
                    // preserves variance below in the `Type::Function` arm. Making
                    // the entire property invariant here erased that distinction, so
                    // a method returning `T` could be treated like a consumer of `T`.
                    // Method bivariance for assignability is handled by relation
                    // checking; it must not flatten the candidate directions here.
                    let property_variance = variance;
                    collect_inference_candidates_inner(
                        arena,
                        param_prop.type_id,
                        arg_prop.type_id,
                        locked,
                        property_variance,
                        false,
                        out,
                        cache,
                        seen,
                    );
                }
            }
            for (signature, from) in index_inference_sources(&param_object, &arg_object) {
                collect_inference_candidates_inner(
                    arena, signature, from, locked, variance, false, out, cache, seen,
                );
            }
        }
        Type::Function(param_fn) => {
            let Type::Function(arg_fn) = arena.get(arg_type).clone() else {
                return;
            };
            // A generic function passed as the argument (`h(f)` with `f<T>(v: T)`): its own type
            // parameters are inferred from what the callback is called with, as tsc does before
            // reading the result. `f` against `(x: number) => R` becomes `(v: number) => number`.
            let arg_fn = if arg_fn.type_params.is_empty() {
                arg_fn
            } else {
                let own: Vec<_> = arg_fn.type_params.iter().map(|p| p.id).collect();
                let mut found: Vec<RawInferenceCandidate> = Vec::new();
                let mut inner_seen = HashSet::default();
                for (argument, parameter) in arg_fn.params.iter().zip(&param_fn.params) {
                    if !contains_type_param(arena, parameter.type_id) {
                        collect_inference_candidates_inner(
                            arena,
                            argument.type_id,
                            parameter.type_id,
                            &[],
                            InferenceVariance::Covariant,
                            true,
                            &mut found,
                            cache,
                            &mut inner_seen,
                        );
                    }
                }
                let bindings: Vec<_> = own
                    .iter()
                    .filter_map(|id| {
                        found
                            .iter()
                            .find(|candidate| candidate.0 == *id)
                            .map(|candidate| (*id, candidate.1))
                    })
                    .collect();
                if bindings.len() != own.len() {
                    // Not every parameter could be read from the callback's parameters: leave
                    // the argument as it is rather than guess.
                    arg_fn
                } else {
                    let params = arg_fn
                        .params
                        .iter()
                        .map(|param| {
                            let mut param = param.clone();
                            param.type_id = substitute_type_params(arena, param.type_id, &bindings);
                            param
                        })
                        .collect();
                    let return_type = substitute_type_params(arena, arg_fn.return_type, &bindings);
                    crate::types::FunctionType {
                        params,
                        return_type,
                        type_params: Vec::new(),
                        is_untyped: arg_fn.is_untyped,
                    }
                }
            };
            let mut nested_locked = locked.to_vec();
            for parameter in &param_fn.type_params {
                if !nested_locked.contains(&parameter.id) {
                    nested_locked.push(parameter.id);
                }
            }
            let tuple_rest = rest_parameter_standing_for_tuple(arena, &param_fn.params);
            let positions = match tuple_rest {
                Some(index) => index,
                None => param_fn.params.len().max(arg_fn.params.len()),
            };
            for position in 0..positions {
                if let (Some(parameter), Some(argument)) = (
                    crate::subtyping::param_type_at(arena, &param_fn.params, position),
                    crate::subtyping::param_type_at(arena, &arg_fn.params, position),
                ) {
                    collect_inference_candidates_inner(
                        arena,
                        parameter,
                        argument,
                        &nested_locked,
                        variance.reversed(),
                        false,
                        out,
                        cache,
                        seen,
                    );
                }
            }
            if let Some(index) = tuple_rest {
                let tuple = remaining_parameters_as_tuple(arena, &arg_fn.params, index);
                collect_inference_candidates_inner(
                    arena,
                    param_fn.params[index].type_id,
                    tuple,
                    &nested_locked,
                    variance.reversed(),
                    false,
                    out,
                    cache,
                    seen,
                );
            }
            collect_inference_candidates_inner(
                arena,
                param_fn.return_type,
                arg_fn.return_type,
                &nested_locked,
                variance,
                false,
                out,
                cache,
                seen,
            );
        }
        Type::Intersection(members) => {
            for member in members {
                if contains_type_param(arena, member) {
                    collect_inference_candidates_inner(
                        arena, member, arg_type, locked, variance, top_level, out, cache, seen,
                    );
                }
            }
        }
        Type::Union(param_members) => {
            let arg_members = match arena.get(arg_type) {
                Type::Union(members) => members.clone(),
                _ => vec![arg_type],
            };
            let remaining: Vec<TypeId> = arg_members
                .into_iter()
                .filter(|&member| {
                    if matches!(arena.get(member), Type::Any | Type::Error) {
                        return true;
                    }
                    !param_members.iter().any(|&concrete| {
                        !contains_type_param(arena, concrete)
                            && is_subtype_via_cache(arena, cache, member, concrete)
                    })
                })
                .collect();
            if remaining.is_empty() {
                return;
            }
            let remainder = if remaining.len() == 1 {
                remaining[0]
            } else {
                arena.alloc_union(remaining)
            };
            for member in param_members {
                if contains_type_param(arena, member) {
                    collect_inference_candidates_inner(
                        arena, member, remainder, locked, variance, top_level, out, cache, seen,
                    );
                }
            }
        }
        _ => {}
    }
}

// Collects the free type parameters appearing in type_id, in declaration order where their
// identity provides one. An explicit call-site type argument list (identity<string, number>(x, y))
// has no declaration span of its own to key a TypeParameterId lookup off of -- it is just a
// positional list -- so the only way to zip it against a function's own T, U, ... is to recover
// the order they were declared in. That order survives in the ids: push_type_params numbers each
// parameter by its position in the source `<...>` list (TypeParameterId::parameter_index).
// Signature-owned parameters are stored explicitly on FunctionType and are supplied by the caller
// before this walk, so only the newly discovered parameters are sorted here. Sorting the caller's
// existing output would interleave indices from unrelated declarations (a method's U and its
// enclosing class's T can both have index zero).
pub fn ordered_generic_param_ids(
    arena: &TypeArena,
    type_id: TypeId,
    out: &mut Vec<crate::types::TypeParameterId>,
) {
    // Most callees are not generic, and the arena remembers that per type, so the common case
    // costs one lookup instead of a walk (this runs for every call expression, on each
    // parameter and the return type).
    if !contains_type_param(arena, type_id) {
        return;
    }
    let mut found: Vec<crate::types::TypeParameterId> = free_generic_params(arena, type_id)
        .iter()
        .map(|&(id, _)| id)
        .collect();
    found.sort_by_key(|id| id.parameter_index());
    for id in found {
        if !out.contains(&id) {
            out.push(id);
        }
    }
}

// The type an argument at index is checked against. A rest parameter absorbs
// every position past the end of the declared list, and its own declared type is
// the array type, not the per-argument type, so it is unwrapped to the element
// type here.
pub fn expected_param_type(
    arena: &crate::arena::TypeArena,
    params: &[crate::types::Param],
    index: usize,
) -> Option<TypeId> {
    let param = match params.get(index) {
        Some(param) => param,
        None => params.last().filter(|p| p.rest)?,
    };
    Some(if param.rest {
        match arena.get(param.type_id) {
            Type::Array(element) => *element,
            // `...args: [string, number?]`: the argument at this index is the tuple
            // element at the same offset into the rest, or its rest element past the end.
            Type::Tuple(elements) => {
                let offset = index.saturating_sub(params.len() - 1);
                let rest = elements.last().filter(|e| e.rest);
                let fixed = elements.len() - usize::from(rest.is_some());
                match elements.get(offset).filter(|_| offset < fixed) {
                    Some(element) => element.type_id,
                    None => rest?.type_id,
                }
            }
            _ => param.type_id,
        }
    } else {
        param.type_id
    })
}

// Like the parameter collector, this sees constraints belonging to free parameters but not
// constraints declared by a nested generic function's own binders: those are checked when that
// nested signature is instantiated, not when the enclosing function is called. It reads the
// same free set (see free_generic_params), keeping a constraint for each constrained parameter.
// A parameter found more than once (T appearing in two parameter positions, for instance) is
// only recorded once, since its constraint is the same wherever it appears. The name travels
// alongside the constraint purely so the diagnostic that checks these can name the offending
// type parameter; it plays no role in matching.
pub fn collect_generic_param_constraints(
    arena: &TypeArena,
    type_id: TypeId,
    constraints: &mut Vec<(crate::types::TypeParameterId, String, TypeId)>,
) {
    if !contains_type_param(arena, type_id) {
        return;
    }
    for &(id, node) in free_generic_params(arena, type_id).iter() {
        if let Type::GenericParameter(_, name, Some(constraint)) = arena.get(node) {
            if !constraints.iter().any(|(existing, _, _)| *existing == id) {
                constraints.push((id, name.clone(), *constraint));
            }
        }
    }
}

/// The (parameter value type, argument type) pairs an index signature contributes to
/// inference: the argument's own signature if it has one, otherwise every property type
/// of the argument (an object literal argument is read as a dictionary of its members).
pub(super) fn index_inference_sources(
    param: &ObjectType,
    arg: &ObjectType,
) -> Vec<(TypeId, TypeId)> {
    let mut pairs = Vec::new();
    for (signature, own) in [
        (&param.string_index, &arg.string_index),
        (&param.number_index, &arg.number_index),
    ] {
        let Some(signature) = signature else { continue };
        match own {
            Some(own) => pairs.push((signature.value, own.value)),
            None => {
                for property in arg.properties.iter() {
                    pairs.push((signature.value, property.type_id));
                }
            }
        }
    }
    pairs
}

/// A variadic parameter tuple with exactly one generic spread, matched against an argument
/// tuple: the (parameter, argument) pairs of the fixed elements before and after the spread,
/// and the spread operand with the tuple of the argument elements between them.
pub(super) fn variadic_split(
    arena: &mut TypeArena,
    param: &[crate::types::TupleElement],
    arg: &[crate::types::TupleElement],
) -> Option<(Vec<(TypeId, TypeId)>, (TypeId, TypeId))> {
    let mut spreads = param.iter().enumerate().filter(|(_, e)| e.rest);
    let (at, spread) = spreads.next()?;
    if spreads.next().is_some() {
        return None;
    }
    let after = param.len() - at - 1;
    if arg.len() < at + after || arg.iter().any(|e| e.rest) {
        return None;
    }
    let mut pairs = Vec::new();
    for index in 0..at {
        pairs.push((param[index].type_id, arg[index].type_id));
    }
    for offset in 0..after {
        pairs.push((
            param[at + 1 + offset].type_id,
            arg[arg.len() - after + offset].type_id,
        ));
    }
    let middle = arena.alloc_tuple(arg[at..arg.len() - after].to_vec());
    Some((pairs, (spread.type_id, middle)))
}
