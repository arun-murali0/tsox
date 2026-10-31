use crate::arena::{TypeArena, TypeId};
use crate::fxhash::{FxBuildHasher, FxHashMap};
use crate::semantic::queries::{RelationCache, SemanticQueries};
use crate::types::{
    ConditionalType, FunctionType, FunctionTypeParam, ObjectType, PropertyEntry, Type,
    TypeParameterId,
};
use smallvec::SmallVec;
use std::collections::HashSet;
use std::rc::Rc;

// Structural inference: walks a generic function's declared parameter type
// alongside a call's actual argument type in lockstep, recording a binding the
// first time it reaches a GenericParameter leaf. This is only the argument-driven
// part: an explicit call-site list like identity<string>(x) is applied by the
// caller (calls.rs), which pre-binds those parameters and passes them in as
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
// per-argument assignability check in calls.rs, the same way it always was.
pub(crate) fn infer_type_param_bindings(
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
fn infer_conditional_type_params(
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

fn collect_conditional_infer_candidates(
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
        && pattern_slot == actual_slot
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
                let count = pattern_function
                    .params
                    .len()
                    .max(actual_function.params.len());
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
pub(crate) fn is_subtype_via_cache(
    arena: &TypeArena,
    cache: &mut RelationCache,
    sub: TypeId,
    sup: TypeId,
) -> bool {
    SemanticQueries::new(arena, cache).is_subtype(sub, sup)
}

// Whether a bound asks for narrow types: it is, or contains, a primitive or a literal.
fn bound_keeps_literals(arena: &TypeArena, bound: TypeId) -> bool {
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
        _ => is_primitive_or_literal(bound),
    }
}

// Which primitive a literal, or a union made only of literals, belongs to. None for
// anything else, and for a union that mixes primitives.
fn literal_base(arena: &TypeArena, id: TypeId) -> Option<u8> {
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

fn same_literal_base(arena: &TypeArena, a: TypeId, b: TypeId) -> bool {
    matches!((literal_base(arena, a), literal_base(arena, b)), (Some(x), Some(y)) if x == y)
}

/// Merge one already-collected inference candidate into a type parameter's chosen
/// binding. Kept in one place so the structural collector and the new candidate
/// finalizer use identical subtype/literal-widening policy.
pub(crate) fn merge_inference_candidate(
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
pub(crate) fn merge_inference_candidate_with_constraint(
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
type SeenPairs = HashSet<(TypeId, TypeId), FxBuildHasher>;

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
fn infer_type_param_bindings_inner(
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

fn infer_type_param_bindings_uncached(
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
        && param_slot == arg_slot
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
            let checked_positions = param_fn.params.len().max(arg_fn.params.len());
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
pub(crate) enum InferenceVariance {
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
pub(crate) type RawInferenceCandidate = (
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

pub(crate) fn collect_inference_candidates(
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

fn collect_inference_candidates_inner(
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
        && param_slot == arg_slot
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
            let candidate = if preserves_literals {
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
        }
        Type::Function(param_fn) => {
            let Type::Function(arg_fn) = arena.get(arg_type).clone() else {
                return;
            };
            let mut nested_locked = locked.to_vec();
            for parameter in &param_fn.type_params {
                if !nested_locked.contains(&parameter.id) {
                    nested_locked.push(parameter.id);
                }
            }
            let positions = param_fn.params.len().max(arg_fn.params.len());
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

// Rebuilds type_id, replacing every GenericParameter occurrence with its bound
// type, or unknown for a parameter nothing ever informed. Bails out immediately
// when type_id contains no type parameters at all, which is every ordinary,
// non-generic type, so this costs nothing beyond one quick tree walk for the
// common case. The function's own stored signature is never mutated; every call
// with different bindings produces a fresh, independent result.
pub(crate) fn substitute_type_params(
    arena: &mut TypeArena,
    type_id: TypeId,
    bindings: &[(crate::types::TypeParameterId, TypeId)],
) -> TypeId {
    substitute_impl(arena, type_id, bindings, false)
}

// Like substitute_type_params, but a parameter with no binding is left as it is
// instead of becoming unknown. A type reference such as `Box<number>` binds only
// the declaration's own parameters, so a method with a parameter of its own
// (`map<U>(f: (x: T) => U): U`) must keep its U to be inferred at the call site.
pub(crate) fn substitute_bound_type_params(
    arena: &mut TypeArena,
    type_id: TypeId,
    bindings: &[(crate::types::TypeParameterId, TypeId)],
) -> TypeId {
    substitute_impl(arena, type_id, bindings, true)
}

/// Reduces a conditional type when its check and extends types are concrete.
/// Otherwise the symbolic node is retained and revisited by generic substitution.
/// This covers distributive conditionals and the structural `infer` patterns most
/// commonly used by utility types without pretending an unresolved T is concrete
/// merely because its constraint happens to fit.
pub(crate) fn resolve_conditional_type(
    arena: &mut TypeArena,
    conditional: ConditionalType,
) -> TypeId {
    let infer_params = &conditional.infer_params;
    if has_unbound_parameters(arena, conditional.check_type, &[])
        || has_unbound_parameters(arena, conditional.extends_type, infer_params)
    {
        return arena.alloc(Type::Conditional(conditional));
    }

    // `any` makes a conditional result include both possible branches. For an
    // infer pattern, TypeScript's any recovery is itself any rather than leaking
    // a locally scoped infer variable into the result.
    if matches!(arena.get(conditional.check_type), Type::Any) {
        if !infer_params.is_empty() {
            return arena.any();
        }
        let yes = substitute_distributive_parameter(
            arena,
            conditional.true_type,
            conditional.distributive_param,
            conditional.check_type,
        );
        let no = substitute_distributive_parameter(
            arena,
            conditional.false_type,
            conditional.distributive_param,
            conditional.check_type,
        );
        return arena.alloc_union(vec![yes, no]);
    }

    if conditional.distributive_param.is_some() {
        match arena.get(conditional.check_type).clone() {
            Type::Never => return arena.never(),
            Type::Union(members) => {
                let mut results = Vec::with_capacity(members.len());
                for member in members {
                    results.push(evaluate_conditional_branch(arena, member, &conditional));
                }
                return arena.alloc_union(results);
            }
            _ => {}
        }
    }

    evaluate_conditional_branch(arena, conditional.check_type, &conditional)
}

fn has_unbound_parameters(arena: &TypeArena, type_id: TypeId, bound: &[TypeParameterId]) -> bool {
    free_generic_params(arena, type_id)
        .iter()
        .any(|(id, _)| !bound.contains(id))
}

fn substitute_distributive_parameter(
    arena: &mut TypeArena,
    branch: TypeId,
    parameter: Option<TypeParameterId>,
    concrete: TypeId,
) -> TypeId {
    let Some(parameter) = parameter else {
        return branch;
    };
    substitute_bound_type_params(arena, branch, &[(parameter, concrete)])
}

fn evaluate_conditional_branch(
    arena: &mut TypeArena,
    check_type: TypeId,
    conditional: &ConditionalType,
) -> TypeId {
    let mut infer_bindings: Vec<(TypeParameterId, TypeId)> = Vec::new();
    if !conditional.infer_params.is_empty() {
        let mut relation_cache = RelationCache::default();
        infer_conditional_type_params(
            arena,
            conditional.extends_type,
            check_type,
            &conditional.infer_params,
            &mut infer_bindings,
            &mut relation_cache,
        );
    }

    if !infer_constraints_hold(arena, conditional, &infer_bindings) {
        return substitute_distributive_parameter(
            arena,
            conditional.false_type,
            conditional.distributive_param,
            check_type,
        );
    }

    let extends_type =
        substitute_bound_type_params(arena, conditional.extends_type, &infer_bindings);
    let extends = if arena.is_promise(check_type) && arena.is_promise(extends_type) {
        match (
            arena.promise_value_type(check_type),
            arena.promise_value_type(extends_type),
        ) {
            (Some(actual), Some(expected)) => crate::subtyping::is_subtype(arena, actual, expected),
            _ => false,
        }
    } else if let (Some(actual), Some(expected)) = (
        arena.record_value_type(check_type),
        arena.record_value_type(extends_type),
    ) {
        // Record is currently represented by an opaque object whose value type lives
        // on the application rather than in the object's members. Preserve that piece
        // of its shape for conditional compatibility; key restrictions remain unmodeled.
        crate::subtyping::is_subtype(arena, actual, expected)
    } else {
        crate::subtyping::is_subtype(arena, check_type, extends_type)
    };
    if !extends {
        return substitute_distributive_parameter(
            arena,
            conditional.false_type,
            conditional.distributive_param,
            check_type,
        );
    }

    let mut branch_bindings = Vec::with_capacity(infer_bindings.len() + 1);
    if let Some(parameter) = conditional.distributive_param {
        branch_bindings.push((parameter, check_type));
    }
    for (id, ty) in infer_bindings {
        if let Some(existing) = branch_bindings.iter_mut().find(|(bound, _)| *bound == id) {
            existing.1 = ty;
        } else {
            branch_bindings.push((id, ty));
        }
    }
    substitute_bound_type_params(arena, conditional.true_type, &branch_bindings)
}

fn infer_constraints_hold(
    arena: &mut TypeArena,
    conditional: &ConditionalType,
    infer_bindings: &[(TypeParameterId, TypeId)],
) -> bool {
    for (id, node) in free_generic_params(arena, conditional.extends_type)
        .iter()
        .copied()
    {
        if !conditional.infer_params.contains(&id) {
            continue;
        }
        let Type::GenericParameter(_, _, Some(constraint)) = arena.get(node).clone() else {
            continue;
        };
        let Some((_, inferred)) = infer_bindings.iter().find(|(bound, _)| *bound == id) else {
            continue;
        };
        let resolved_constraint = substitute_bound_type_params(arena, constraint, infer_bindings);
        let inferred_ids: Vec<TypeParameterId> =
            infer_bindings.iter().map(|(bound, _)| *bound).collect();
        if !has_unbound_parameters(arena, resolved_constraint, &inferred_ids)
            && !crate::subtyping::is_subtype(arena, *inferred, resolved_constraint)
        {
            return false;
        }
    }
    true
}

fn substitute_impl(
    arena: &mut TypeArena,
    type_id: TypeId,
    bindings: &[(crate::types::TypeParameterId, TypeId)],
    keep_unbound: bool,
) -> TypeId {
    // Nothing bound, nothing to rewrite: the id comes back as it is and no scratch state
    // is built.
    if bindings.is_empty() {
        return type_id;
    }
    let mut outermost_cut = usize::MAX;
    Substitution {
        bindings,
        keep_unbound,
        preserve: Vec::new(),
        open: Vec::new(),
        rewritten: FxHashMap::default(),
        closing: FxHashMap::default(),
        cyclic: Vec::new(),
    }
    .rewrite(arena, type_id, &mut outermost_cut)
}

// Scratch state for one substitute_impl call, so that everything learned while
// rewriting one part of a type is reused by the rest of it. The graph is a DAG
// after interning, so a subtype reachable along several paths is rewritten once
// (`rewritten`) instead of once per path, and "does this contain a parameter" is
// answered from the arena's own cache (see contains_type_param) instead of a fresh
// walk at every level.
//
// `rewritten` does not outlive the call. A declaration's Ref can be resolved by
// TypeArena::resolve_ref between two calls, which would make a longer-lived rewrite
// stale.
// How deep one substitution may go before it is cut (see `Substitution::rewrite`). Real types are a
// few dozen levels deep; tsc stops an instantiation at 100.
const MAX_INSTANTIATION_DEPTH: usize = 2_000;

struct Substitution<'b> {
    bindings: &'b [(crate::types::TypeParameterId, TypeId)],
    keep_unbound: bool,
    // Parameters temporarily held back while a distributive conditional's branch
    // types are rewritten. Each union constituent gets its own replacement later.
    preserve: Vec<crate::types::TypeParameterId>,

    // Ids currently being rewritten, outermost first.
    open: Vec<TypeId>,

    rewritten: FxHashMap<TypeId, TypeId>,

    // The Ref `rewrite_recursive` has opened for an id and not yet resolved, while its
    // children are being walked. Kept apart from `rewritten` because a conditional empties
    // `rewritten` for its branches, and the edge that closes a recursive conditional alias
    // (`Flatten<T> = T extends (infer U)[] ? Flatten<U> : T`) is in a branch: with the
    // entry gone it found nothing, started the first and second pass over again, and never
    // came back.
    closing: FxHashMap<TypeId, TypeId>,

    // Ids the walk has found to reach themselves (see rewrite).
    cyclic: Vec<TypeId>,
}

impl Substitution<'_> {
    // A recursive type (Node { next: Node }, or a recursive generic like
    // `Box<T> { value: T, next: Box<T> | null }`) means substituting inside
    // type_id's properties can lead back to substituting type_id itself before the
    // first call returns. Re-entering an id already on the current path is exactly
    // that self-referential edge, and it is what stops the walk from looping.
    //
    // Leaving the edge pointing at the original id is right for a type that has
    // nothing to substitute, and wrong for one that does: `Box<number>.next` would
    // still be the generic `Box<T> | null`, and `box.next.value` would read as T.
    // So the cut only *finds* the cycle (`cyclic`); once the first pass over an
    // object has finished and it turns out to reach itself, rewrite_recursive
    // builds it again with a Ref of its own, so that the edge closes on the new object
    // instead. Only an Object can close a cycle (see TypeArena), which
    // is why nothing else gets a second pass.
    //
    // `lowest_cut` is the index in `open` of the outermost id any cut below this
    // call pointed at (usize::MAX when none did). A result is memoised only when
    // no cut reached outside the node: a rewrite that left an outer id as-is
    // reflects where the walk entered, and reusing it from another entry point
    // could differ from what a fresh walk would build there.
    fn rewrite(
        &mut self,
        arena: &mut TypeArena,
        type_id: TypeId,
        lowest_cut: &mut usize,
    ) -> TypeId {
        if let Type::GenericParameter(id, _, _) = arena.get(type_id)
            && self.preserve.contains(id)
        {
            return type_id;
        }
        if let Some(&done) = self.rewritten.get(&type_id) {
            return done;
        }
        if let Some(&copy) = self.closing.get(&type_id) {
            return copy;
        }
        if !contains_type_param(arena, type_id) {
            return type_id;
        }
        if let Some(position) = self.open.iter().position(|&id| id == type_id) {
            *lowest_cut = (*lowest_cut).min(position);
            if !self.cyclic.contains(&type_id) {
                self.cyclic.push(type_id);
            }
            return type_id;
        }

        // Problem: a type that a generic declaration builds one level at a time can be deeper
        // than anything its text shows, and substituting into it recurses once per level.
        // Picked: stop past a fixed depth, or when the stack budget is gone, and hand back the
        // error type, which does not cascade into a mismatch. The unit says so once (TSR9102).
        // A count, not only the stack, so the cut point is the same in a debug and a release
        // build and on any thread.
        if self.open.len() >= MAX_INSTANTIATION_DEPTH || crate::stack_guard::exhausted() {
            crate::stack_guard::note_cut();
            return arena.error();
        }
        let depth = self.open.len();
        self.open.push(type_id);
        let mut cut_below = usize::MAX;
        let mut result = self.rewrite_uncached(arena, type_id, &mut cut_below);
        self.open.pop();

        if self.cyclic.contains(&type_id)
            && matches!(
                arena.get(type_id),
                Type::Object(_)
                    | Type::Array(_)
                    | Type::Function(_)
                    | Type::Union(_)
                    | Type::Intersection(_)
                    | Type::Conditional(_)
            )
        {
            // The first pass left a recursive edge on the original node, so its result
            // is discarded. Close the cycle around a Ref for every recursive structural
            // shape, not just objects: recursive conditional aliases and unions need the
            // same bounded graph rewrite as linked-list objects.
            cut_below = usize::MAX;
            result = self.rewrite_recursive(arena, type_id, &mut cut_below);
        }

        if cut_below >= depth {
            self.rewritten.insert(type_id, result);
        } else {
            // rewrite_recursive registers its Ref before descending; when
            // the result turns out not to be reusable, that entry must go too.
            self.rewritten.remove(&type_id);
        }
        *lowest_cut = (*lowest_cut).min(cut_below);
        result
    }

    // Rebuilds a recursive structural node around a Ref of its own. The Ref is
    // registered as type_id's rewrite before any child is walked, so the edge that closed
    // the first pass closes on this substituted copy. Object aliases were the first use;
    // array, union, function and conditional nodes also need this for recursive aliases.
    fn rewrite_recursive(
        &mut self,
        arena: &mut TypeArena,
        type_id: TypeId,
        cut: &mut usize,
    ) -> TypeId {
        let shape = arena.get(type_id).clone();
        if !matches!(
            &shape,
            Type::Object(_)
                | Type::Array(_)
                | Type::Function(_)
                | Type::Union(_)
                | Type::Intersection(_)
                | Type::Conditional(_)
        ) {
            return type_id;
        }

        let copy = arena.alloc_ref();
        self.rewritten.insert(type_id, copy);
        self.closing.insert(type_id, copy);
        let body = self.rewrite_shape(arena, type_id, shape, cut);
        self.closing.remove(&type_id);
        // A reducible conditional can collapse directly to its own placeholder. A Ref
        // cannot have itself as its body, so retain the symbolic conditional in that
        // exceptional self-result case rather than creating a wrapper-unwrapping loop.
        let body = if body == copy {
            match arena.get(type_id).clone() {
                Type::Conditional(conditional) => arena.alloc(Type::Conditional(conditional)),
                _ => body,
            }
        } else {
            body
        };
        if arena.slot_of(body).is_some() {
            // Never resolve one Ref to another. The placeholder is still usable as the
            // conservative result when the graph has no concrete body to close on.
            self.rewritten.insert(type_id, type_id);
            return type_id;
        }
        arena.resolve_ref(copy, body);
        copy
    }

    fn rewrite_uncached(
        &mut self,
        arena: &mut TypeArena,
        type_id: TypeId,
        cut: &mut usize,
    ) -> TypeId {
        if let Type::App(slot, arguments, body) = arena.raw(type_id).clone() {
            let template = arena.app_template(type_id).cloned();
            let rewritten_arguments: Vec<TypeId> = arguments
                .iter()
                .map(|&argument| self.rewrite(arena, argument, cut))
                .collect();

            if let Some(template) = template {
                if rewritten_arguments != arguments {
                    // This App was first encountered while its own declaration was still
                    // resolving, so its body is a snapshot of the declaration specialized
                    // with symbolic arguments. Once an infer variable or outer generic
                    // changes those arguments, rewrite the declaration body again with the
                    // new bindings; substituting the stale body alone leaves recursive
                    // conditionals such as Flatten<T> stuck at `Flatten<infer U>`.
                    let bindings: Vec<(TypeParameterId, TypeId)> = template
                        .parameters
                        .iter()
                        .copied()
                        .zip(rewritten_arguments.iter().copied())
                        .collect();
                    if !arena.begin_app_instantiation(template.base, &bindings) {
                        crate::stack_guard::note_cut();
                        return arena.error();
                    }
                    let result = substitute_bound_type_params(arena, template.base, &bindings);
                    arena.end_app_instantiation(template.base, &bindings);
                    return result;
                }

                // Unchanged arguments leave the application as it is. Its body is the
                // declaration specialized with exactly those arguments, so it has no free
                // parameter of its own to substitute, and walking it only finds the
                // declaration again through its own recursive step: the cycle that
                // `Flatten<T> = T extends (infer U)[] ? Flatten<U> : T` is made of.
                return type_id;
            }

            let body = self.rewrite(arena, body, cut);
            return arena.alloc_app(slot, rewritten_arguments, body);
        }

        let shape = arena.get(type_id).clone();
        self.rewrite_shape(arena, type_id, shape, cut)
    }

    fn rewrite_shape(
        &mut self,
        arena: &mut TypeArena,
        original: TypeId,
        shape: Type,
        cut: &mut usize,
    ) -> TypeId {
        match shape {
            Type::GenericParameter(id, _, _) => {
                if self.preserve.contains(&id) {
                    return original;
                }
                let bound = self
                    .bindings
                    .iter()
                    .find(|(bound, _)| *bound == id)
                    .map(|(_, resolved)| *resolved);
                match bound {
                    Some(resolved) => resolved,
                    None if self.keep_unbound => original,
                    None => arena.unknown(),
                }
            }
            Type::Array(element) => {
                let substituted = self.rewrite(arena, element, cut);
                arena.alloc(Type::Array(substituted))
            }
            Type::Tuple(elements) => {
                let mut rewritten = Vec::with_capacity(elements.len());
                for element in &elements {
                    rewritten.push(crate::types::TupleElement {
                        type_id: self.rewrite(arena, element.type_id, cut),
                        optional: element.optional,
                        rest: element.rest,
                    });
                }
                arena.alloc_tuple(rewritten)
            }
            Type::Function(function) => {
                let mut params = Vec::with_capacity(function.params.len());
                for p in &function.params {
                    params.push(crate::types::Param {
                        type_id: self.rewrite(arena, p.type_id, cut),
                        optional: p.optional,
                        rest: p.rest,
                        name: p.name.clone(),
                    });
                }
                let return_type = self.rewrite(arena, function.return_type, cut);
                let type_params = function
                    .type_params
                    .iter()
                    .map(|param| FunctionTypeParam {
                        id: param.id,
                        name: param.name.clone(),
                        constraint: param.constraint.map(|id| self.rewrite(arena, id, cut)),
                        default: param.default.map(|id| self.rewrite(arena, id, cut)),
                    })
                    .collect();
                arena.alloc(Type::Function(FunctionType {
                    params,
                    return_type,
                    type_params,
                    is_untyped: function.is_untyped,
                }))
            }
            Type::Object(object) => {
                let mut properties = Vec::with_capacity(object.properties.len());
                for p in object.properties.iter() {
                    properties.push(PropertyEntry {
                        name: p.name.clone(),
                        type_id: self.rewrite(arena, p.type_id, cut),
                        optional: p.optional,
                        is_method: p.is_method,
                    });
                }
                arena.alloc(Type::Object(ObjectType::new(properties)))
            }
            Type::Union(members) => {
                let mut substituted = Vec::with_capacity(members.len());
                for &m in &members {
                    substituted.push(self.rewrite(arena, m, cut));
                }
                arena.alloc_union(substituted)
            }
            // Rebuilt through alloc_intersection, not allocated as it stands: replacing a
            // parameter can create something that reduces (`T & number` with `T := string`
            // is `never`), and a node that skipped the reduction would not be canonical.
            // Distributing over a union that is too large cannot be reported from here,
            // so it becomes the error type, which is already reported where it was written.
            Type::Intersection(members) => {
                let mut substituted = Vec::with_capacity(members.len());
                for &m in &members {
                    substituted.push(self.rewrite(arena, m, cut));
                }
                arena
                    .alloc_intersection(substituted)
                    .unwrap_or_else(|_| arena.error())
            }
            Type::Conditional(conditional) => {
                // An outer type argument may make this conditional decidable. Keep
                // unbound parameters inside the symbolic node until its check side
                // and extends pattern have been substituted, then reduce it once.
                let previous_keep_unbound = self.keep_unbound;
                self.keep_unbound = true;
                let check_type = self.rewrite(arena, conditional.check_type, cut);

                // The `infer` parameters are binders of this conditional: they are in
                // scope in the `extends` pattern and the true branch, and nowhere else.
                // A recursive alias (`Flatten<T> = T extends (infer Item)[] ? Flatten<Item>
                // : T`) rebuilds its body one level at a time with the same parameter ids,
                // so this conditional's check type can mention an outer `Item` while its own
                // pattern binds `Item` again. Substituting the outer `Item` into the
                // pattern as well turned `(infer Item)[]` into `number[][]` and stopped
                // the recursion one level early. They are held back here for the pattern
                // and the true branch, and not for the check type above.
                let binders_held = conditional.infer_params.len();
                self.preserve
                    .extend(conditional.infer_params.iter().copied());

                // Everything below uses its own memo: a type rewritten while the binders
                // are held, or while a distributive parameter is, is not the same rewrite
                // as one made outside. Otherwise an `extends` pattern that also contains
                // `T[]` could cache `T[] -> (string | number)[]` before the distributive
                // branch gets to preserve T, silently turning `T extends unknown ? T[] :
                // never` into an array of the entire union. Discarded afterward.
                let outer_rewrites = std::mem::take(&mut self.rewritten);
                let extends_type = self.rewrite(arena, conditional.extends_type, cut);

                if let Some(parameter) = conditional.distributive_param {
                    self.preserve.push(parameter);
                }
                let true_type = self.rewrite(arena, conditional.true_type, cut);
                // The false branch is outside the binders' scope.
                let keep = self.preserve.len()
                    - binders_held
                    - usize::from(conditional.distributive_param.is_some());
                let held: Vec<_> = self.preserve.drain(keep..).collect();
                if let Some(parameter) = conditional.distributive_param {
                    self.preserve.push(parameter);
                }
                let false_type = self.rewrite(arena, conditional.false_type, cut);
                if conditional.distributive_param.is_some() {
                    self.preserve.pop();
                }
                drop(held);
                self.rewritten = outer_rewrites;
                self.keep_unbound = previous_keep_unbound;

                resolve_conditional_type(
                    arena,
                    ConditionalType {
                        check_type,
                        extends_type,
                        true_type,
                        false_type,
                        infer_params: conditional.infer_params,
                        distributive_param: conditional.distributive_param,
                    },
                )
            }
            _ => original,
        }
    }
}

// The free type parameters of a type: every GenericParameter reachable from it that is not
// declared by a generic function type on the way down. `<U>(x: U) => U` has none; its U is
// bound. `<U>(x: U, y: T) => U` has T. Each element is the parameter's id and the
// GenericParameter node that names it, so one walk answers three questions: is the type
// generic at all (contains_type_param), which parameters does it need explicit type
// arguments for, in declaration order (ordered_generic_param_ids), and which of them carry
// an `extends` bound (collect_generic_param_constraints).
//
// Problem 1: an earlier version of this walk carried the list of generic binders it was
// inside and keyed its "already visited" set by (type, binders). A self-referential generic
// signature never repeats that key. `class Chain<T> { then<S>(cb: (x: T) => S): Chain<S> }`
// is two types, Chain<S> and the `then` function, that lead to each other, and every lap
// through `then` pushed S onto the binder list again. gdb on the hung process showed exactly
// two type ids alternating and a binder list that went from 3,841 to 6,118 entries between
// samples: the walk never ended. On TypeScript's own tests that was 15 files that hung or
// ran out of memory (promiseChaining, promisePermutations, chainedCalls...).
// Fix: the free set of a node does not depend on the path that reached it. It is the node's
// own contents minus the binders the node itself declares, so there is no path-dependent
// key left to grow.
//
// Problem 2: the first version of that fix still walked down from the start and cut a
// cycle where it re-entered a node that was still open. An answer cut at an outer open node
// is only partial, so it was not remembered, and every other path to the same node walked
// it again. With classes that point at each other (`class C0 { p1: C1; ... p5: C5 }` and so
// on, as in resolvingClassDeclarationWhenInBaseTypeResolution) the number of paths grows
// exponentially, and the walk never finished.
// Picked: find every type reachable from the start once, then settle the free sets of all of
// them together. A set is the union of its children's sets minus the node's own binders,
// which only ever grows as the children's sets do, so repeating it until nothing changes
// reaches the one right answer for every node at once, cycles included. Each node is
// visited once to find it and a few times to settle it, so the cost follows the size of the
// graph and not the number of paths through it. The answers are exact for every node, so all
// of them are remembered in the arena, not only the ones a cut did not touch.
// Cost: one small vector per node and a few passes over the reachable graph the first time a
// type is asked about; the arena keeps the boolean, so every later question is one lookup. A
// node the arena already knows to hold no parameter is not walked into at all.
type FreeParam = (crate::types::TypeParameterId, TypeId);

fn is_constrained(arena: &TypeArena, node: TypeId) -> bool {
    matches!(arena.get(node), Type::GenericParameter(_, _, Some(_)))
}

// Adds `from` to `into`, one entry per parameter id, first seen first. When a parameter is
// met both as a bare node and as one carrying its constraint, the constrained node is kept:
// the constraint is the same wherever the parameter appears.
fn merge_free(arena: &TypeArena, into: &mut Vec<FreeParam>, from: &[FreeParam]) {
    for &(id, node) in from {
        match into.iter_mut().find(|(existing, _)| *existing == id) {
            None => into.push((id, node)),
            Some(slot) => {
                if !is_constrained(arena, slot.1) && is_constrained(arena, node) {
                    slot.1 = node;
                }
            }
        }
    }
}

// The types a type is made of, in the order their free sets are merged: a generic function's
// own bounds and defaults, then its parameters, then its return type.
fn parts_of(arena: &TypeArena, type_id: TypeId) -> SmallVec<[TypeId; 8]> {
    let mut parts = SmallVec::new();
    match arena.raw(type_id) {
        Type::App(_, arguments, body) => {
            parts.extend(arguments.iter().copied());
            parts.push(*body);
            return parts;
        }
        Type::Named(_, inner) => {
            parts.push(*inner);
            return parts;
        }
        _ => {}
    }
    match arena.get(type_id) {
        Type::Array(element) => parts.push(*element),
        Type::Tuple(elements) => parts.extend(elements.iter().map(|e| e.type_id)),
        Type::Function(function) => {
            for parameter in &function.type_params {
                parts.extend(
                    [parameter.constraint, parameter.default]
                        .into_iter()
                        .flatten(),
                );
            }
            parts.extend(function.params.iter().map(|parameter| parameter.type_id));
            parts.push(function.return_type);
        }
        Type::Object(object) => parts.extend(object.properties.iter().map(|p| p.type_id)),
        Type::Union(members) | Type::Intersection(members) => parts.extend(members.iter().copied()),
        Type::Conditional(conditional) => {
            parts.extend([
                conditional.check_type,
                conditional.extends_type,
                conditional.true_type,
                conditional.false_type,
            ]);
        }
        _ => {}
    }
    parts
}

// Whether two lists name the same parameters, each by the same kind of node. The order is
// left out on purpose: it follows the order the children happened to be settled in, and it
// is the set that decides whether another pass is needed.
fn same_free_set(arena: &TypeArena, a: &[FreeParam], b: &[FreeParam]) -> bool {
    a.len() == b.len()
        && a.iter().all(|&(id, node)| {
            b.iter().any(|&(other, other_node)| {
                other == id && is_constrained(arena, node) == is_constrained(arena, other_node)
            })
        })
}

// The free parameters of `type_id`, in first-seen order. The arena's "mentions a type
// parameter" answer is published for every node reached, so the next contains_type_param on
// any of them is one lookup.
fn free_generic_params(arena: &TypeArena, type_id: TypeId) -> Rc<[FreeParam]> {
    // Every type reachable from the start, each once, in the order it was found. `position`
    // doubles as the visited set.
    let mut nodes: Vec<TypeId> = Vec::new();
    let mut parts: Vec<SmallVec<[usize; 8]>> = Vec::new();
    let mut position: FxHashMap<TypeId, usize> = FxHashMap::default();
    reach(arena, type_id, &mut nodes, &mut parts, &mut position);

    let mut sets: Vec<Vec<FreeParam>> = nodes
        .iter()
        .map(|&node| match arena.get(node) {
            Type::GenericParameter(id, _, _) => vec![(*id, node)],
            _ => Vec::new(),
        })
        .collect();

    // Last found first: a node is found before the parts it is made of, so going backwards
    // settles most parts before the node that uses them, and an acyclic graph is done in
    // one pass plus the pass that sees nothing change.
    loop {
        let mut changed = false;
        for index in (0..nodes.len()).rev() {
            let mut free: Vec<FreeParam> = match arena.get(nodes[index]) {
                Type::GenericParameter(id, _, _) => vec![(*id, nodes[index])],
                _ => Vec::new(),
            };
            for &part in &parts[index] {
                merge_free(arena, &mut free, &sets[part]);
            }
            // The function's own binders are bound inside it, wherever they were used.
            if let Type::Function(function) = arena.get(nodes[index]) {
                free.retain(|(id, _)| !function.type_params.iter().any(|own| own.id == *id));
            }
            if let Type::Conditional(conditional) = arena.get(nodes[index]) {
                free.retain(|(id, _)| !conditional.infer_params.contains(id));
            }
            if !same_free_set(arena, &free, &sets[index]) {
                changed = true;
            }
            sets[index] = free;
        }
        if !changed {
            break;
        }
    }

    for (node, set) in nodes.iter().zip(&sets) {
        arena.remember_mentions_type_param(*node, !set.is_empty());
    }
    Rc::from(sets[position[&type_id]].clone())
}

// Finds `type_id` and everything reachable from it, once each. A type the arena already
// knows to mention no parameter is a leaf: it can only contribute an empty set.
fn reach(
    arena: &TypeArena,
    type_id: TypeId,
    nodes: &mut Vec<TypeId>,
    parts: &mut Vec<SmallVec<[usize; 8]>>,
    position: &mut FxHashMap<TypeId, usize>,
) -> usize {
    if let Some(&known) = position.get(&type_id) {
        return known;
    }
    // Reserved before the parts are visited, so a cycle back to this node finds its slot.
    let slot = nodes.len();
    position.insert(type_id, slot);
    nodes.push(type_id);
    parts.push(SmallVec::new());
    if arena.cached_mentions_type_param(type_id) != Some(false) {
        let children: SmallVec<[usize; 8]> = parts_of(arena, type_id)
            .into_iter()
            .map(|child| reach(arena, child, nodes, parts, position))
            .collect();
        parts[slot] = children;
    }
    slot
}

// Whether a type mentions a *free* type parameter, i.e. one not declared by a generic
// function type inside it. Answers are kept in the arena (TypeArena::cached_mentions_type_param),
// not per call: the checker asks it for every call expression, mostly about types that never
// change, and interning has turned the type graph into a DAG.
pub(crate) fn contains_type_param(arena: &TypeArena, type_id: TypeId) -> bool {
    if let Some(known) = arena.cached_mentions_type_param(type_id) {
        return known;
    }
    !free_generic_params(arena, type_id).is_empty()
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
pub(crate) fn ordered_generic_param_ids(
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
pub(crate) fn expected_param_type(
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
pub(crate) fn collect_generic_param_constraints(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TypeParameterId;

    // Pins the reason this path was rerouted: a second candidate for an already
    // bound parameter is compared against the existing binding, and that comparison
    // has to land in the shared cache or repeated candidates recompute it forever.
    // Neither direction holds between number and string, so both are asked and the
    // first binding survives; two entries is the proof both went through the cache.
    #[test]
    fn competing_candidates_are_compared_through_the_relation_cache() {
        let mut arena = TypeArena::new();
        let id = TypeParameterId::new(0, 0);
        let t = arena.alloc(Type::GenericParameter(id, "T".to_string(), None));
        let (number, string) = (arena.number(), arena.string());
        let mut bindings = Vec::new();
        let mut cache = RelationCache::default();

        infer_type_param_bindings(&mut arena, t, number, &mut bindings, &[], &mut cache);
        infer_type_param_bindings(&mut arena, t, string, &mut bindings, &[], &mut cache);

        assert_eq!(bindings, vec![(id, number)]);
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn substitution_respects_nested_binder_identity_even_when_names_match() {
        let mut arena = TypeArena::new();
        let outer_id = TypeParameterId::new(10, 0);
        let inner_id = TypeParameterId::new(20, 0);
        let outer_t = arena.alloc(Type::GenericParameter(outer_id, "T".into(), None));
        let inner_t = arena.alloc(Type::GenericParameter(inner_id, "T".into(), None));
        let nested = arena.alloc(Type::Function(FunctionType {
            params: vec![crate::types::Param::required(inner_t)],
            return_type: outer_t,
            type_params: vec![FunctionTypeParam {
                id: inner_id,
                name: "T".into(),
                constraint: None,
                default: None,
            }],
            is_untyped: false,
        }));

        let number = arena.number();
        let instantiated = substitute_bound_type_params(&mut arena, nested, &[(outer_id, number)]);
        let Type::Function(instantiated) = arena.get(instantiated) else {
            panic!("expected a function after substitution");
        };
        assert_eq!(instantiated.params[0].type_id, inner_t);
        assert_eq!(instantiated.return_type, number);
        assert_eq!(instantiated.type_params[0].id, inner_id);
    }

    #[test]
    fn inference_matches_parameters_against_a_rest_signature_by_call_position() {
        let mut arena = TypeArena::new();
        let id = TypeParameterId::new(30, 0);
        let t = arena.alloc(Type::GenericParameter(id, "T".into(), None));
        let unknown = arena.unknown();
        let expected_callback = arena.alloc(Type::Function(FunctionType {
            params: vec![
                crate::types::Param::required(unknown),
                crate::types::Param::required(t),
            ],
            return_type: arena.void(),
            type_params: Vec::new(),
            is_untyped: false,
        }));
        let number = arena.number();
        let number_array = arena.alloc(Type::Array(number));
        let actual_callback = arena.alloc(Type::Function(FunctionType {
            params: vec![crate::types::Param {
                type_id: number_array,
                optional: false,
                rest: true,
                name: None,
            }],
            return_type: arena.void(),
            type_params: Vec::new(),
            is_untyped: false,
        }));
        let mut bindings = Vec::new();
        let mut cache = RelationCache::default();

        infer_type_param_bindings(
            &mut arena,
            expected_callback,
            actual_callback,
            &mut bindings,
            &[],
            &mut cache,
        );

        assert_eq!(bindings, vec![(id, number)]);
    }

    #[test]
    fn nested_generic_signature_parameters_are_bound_but_outer_parameters_remain_free() {
        let mut arena = TypeArena::new();
        let outer_id = TypeParameterId::new(10, 0);
        let local_id = TypeParameterId::new(20, 0);
        let outer = arena.alloc(Type::GenericParameter(outer_id, "T".into(), None));
        let local = arena.alloc(Type::GenericParameter(local_id, "U".into(), None));
        let void = arena.void();

        let only_local = arena.alloc(Type::Function(FunctionType {
            params: vec![crate::types::Param::required(local)],
            return_type: void,
            type_params: vec![FunctionTypeParam {
                id: local_id,
                name: "U".into(),
                constraint: None,
                default: None,
            }],
            is_untyped: false,
        }));
        assert!(!contains_type_param(&arena, only_local));
        let mut leaked = Vec::new();
        ordered_generic_param_ids(&arena, only_local, &mut leaked);
        assert!(leaked.is_empty());

        let with_outer = arena.alloc(Type::Function(FunctionType {
            params: vec![crate::types::Param::required(local)],
            return_type: outer,
            type_params: vec![FunctionTypeParam {
                id: local_id,
                name: "U".into(),
                constraint: None,
                default: None,
            }],
            is_untyped: false,
        }));
        assert!(contains_type_param(&arena, with_outer));
        let mut ids = Vec::new();
        ordered_generic_param_ids(&arena, with_outer, &mut ids);
        assert_eq!(ids, vec![outer_id]);
    }

    fn property(name: &str, type_id: TypeId) -> PropertyEntry {
        PropertyEntry {
            name: name.into(),
            type_id,
            optional: false,
            is_method: false,
        }
    }

    fn property_type(arena: &TypeArena, object: TypeId, name: &str) -> TypeId {
        let Type::Object(object) = arena.get(object) else {
            panic!("expected an object");
        };
        object
            .properties
            .iter()
            .find(|p| &*p.name == name)
            .map(|p| p.type_id)
            .unwrap_or_else(|| panic!("no property {name}"))
    }

    // `Box<T> { value: T; next: Box<T> | null }` bound at T = number. The self
    // edge must close on the new object, not stay on the generic original:
    // otherwise `next` would still read as `Box<T> | null` and `box.next.value`
    // would be T. Nothing generic may be reachable from the result.
    #[test]
    fn recursive_object_substitution_closes_on_the_copy() {
        let mut arena = TypeArena::new();
        let id = TypeParameterId::new(0, 0);
        let t = arena.alloc(Type::GenericParameter(id, "T".to_string(), None));
        let number = arena.number();
        let null = arena.null();

        let node = arena.alloc_ref();
        let next = arena.alloc_union(vec![node, null]);
        let body = arena.alloc(Type::Object(ObjectType::new(vec![
            property("value", t),
            property("next", next),
        ])));
        arena.resolve_ref(node, body);

        let bound = substitute_bound_type_params(&mut arena, node, &[(id, number)]);

        assert_ne!(bound, node);
        assert!(!contains_type_param(&arena, bound));
        assert_eq!(property_type(&arena, bound, "value"), number);

        let next = property_type(&arena, bound, "next");
        let Type::Union(members) = arena.get(next).clone() else {
            panic!("expected a union");
        };
        assert!(members.contains(&bound), "the edge must close on the copy");
        assert!(
            !members.contains(&node),
            "the generic original must not leak"
        );
    }

    // A -> B -> A, both mentioning T. Each copy has to point at the other copy,
    // not back at a generic original.
    #[test]
    fn mutually_recursive_substitution_stays_inside_the_copies() {
        let mut arena = TypeArena::new();
        let id = TypeParameterId::new(0, 0);
        let t = arena.alloc(Type::GenericParameter(id, "T".to_string(), None));
        let number = arena.number();

        let a = arena.alloc_ref();
        let b = arena.alloc_ref();
        let b_body = arena.alloc(Type::Object(ObjectType::new(vec![
            property("item", t),
            property("back", a),
        ])));
        arena.resolve_ref(b, b_body);
        let a_body = arena.alloc(Type::Object(ObjectType::new(vec![
            property("item", t),
            property("other", b),
        ])));
        arena.resolve_ref(a, a_body);

        let bound = substitute_bound_type_params(&mut arena, a, &[(id, number)]);

        assert!(!contains_type_param(&arena, bound));
        let other = property_type(&arena, bound, "other");
        assert_ne!(other, b);
        assert_eq!(property_type(&arena, other, "item"), number);
        assert_eq!(property_type(&arena, other, "back"), bound);
    }

    // A recursive type with nothing to substitute comes back untouched: no copy,
    // no second pass.
    #[test]
    fn recursive_object_without_type_parameters_is_returned_as_is() {
        let mut arena = TypeArena::new();
        let id = TypeParameterId::new(0, 0);
        let number = arena.number();
        let null = arena.null();
        let node = arena.alloc_ref();
        let next = arena.alloc_union(vec![node, null]);
        let body = arena.alloc(Type::Object(ObjectType::new(vec![
            property("value", number),
            property("next", next),
        ])));
        arena.resolve_ref(node, body);

        assert_eq!(
            substitute_bound_type_params(&mut arena, node, &[(id, number)]),
            node
        );
    }
}
