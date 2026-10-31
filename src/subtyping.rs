use std::cmp::Ordering;

use crate::arena::{PairStack, TypeArena, TypeId};
use crate::types::{ObjectType, Param, TupleElement, Type, TypeParameterId};

// Structural subtyping: is sub usable wherever sup is expected. The identity check
// comes first since it is the cheapest and covers the common case of comparing a
// type to itself. Any and Error are checked next because they need to short-circuit
// before the big match below; otherwise Error, used as a sentinel for an expression
// that already failed to check, would need its own arm in every single case of that
// match instead of one shared escape hatch here.
pub fn is_subtype(arena: &TypeArena, sub: TypeId, sup: TypeId) -> bool {
    is_subtype_inner(arena, sub, sup, &mut PairStack::new(), &[])
}

// A recursive type (Node { next: Node }, whose declaration's Ref namespace::resolve
// resolves after its members) means comparing sub and sup can lead back to
// comparing the same (sub, sup) pair again before either call has returned.
// `seen` tracks pairs currently on the call stack. Re-entering one is treated
// as true (coinductively: two types that only differ by "going in circles"
// are equivalent) rather than as a fresh comparison to keep making; this is
// the standard rule for equirecursive subtyping and is what breaks the loop.
fn is_subtype_inner(
    arena: &TypeArena,
    sub: TypeId,
    sup: TypeId,
    seen: &mut PairStack,
    binders: &[(TypeParameterId, TypeParameterId)],
) -> bool {
    // Generic function signatures are alpha-equivalent when their corresponding
    // bound parameters differ only by declaration identity. Check this before the
    // ordinary bound-parameter rule below, which would otherwise compare constraints.
    if let (Type::GenericParameter(sub_id, _, _), Type::GenericParameter(sup_id, _, _)) =
        (arena.get(sub), arena.get(sup))
        && binders
            .iter()
            .any(|(a, b)| (*a == *sub_id && *b == *sup_id) || (*a == *sup_id && *b == *sub_id))
    {
        return true;
    }
    if is_trivial_subtype(arena, sub, sup) {
        return true;
    }

    // `Promise<T>` is an application over an empty body (there is no lib to read its
    // members from), so looking through it would make every promise a subtype of every
    // other. It is covariant in T, so two promises are compared by their value types.
    if let (Some(sub_value), Some(sup_value)) =
        (arena.promise_value_type(sub), arena.promise_value_type(sup))
    {
        return is_subtype_inner(arena, sub_value, sup_value, seen, binders);
    }

    let pair = (sub, sup);
    if seen.contains(&pair) {
        return true;
    }
    seen.push(pair);
    let result = is_subtype_uncached(arena, sub, sup, seen, binders);
    seen.pop();
    result
}

fn is_subtype_uncached(
    arena: &TypeArena,
    sub: TypeId,
    sup: TypeId,
    seen: &mut PairStack,
    binders: &[(TypeParameterId, TypeParameterId)],
) -> bool {
    match (arena.get(sub), arena.get(sup)) {
        (_, Type::Unknown) => true,

        // An intersection whose members disagree about a discriminant has no values, so
        // it is `never` and a subtype of everything. It is still an intersection node
        // (see TypeArena::intersection_reduces_to_never for why), which is why this is
        // asked here and not decided when it was built.
        (Type::Intersection(_), _) if arena.intersection_reduces_to_never(sub) => true,

        // A bounded parameter against a union that names it (`T | undefined`) is
        // already a subtype through that member, whatever the bound is. Asked first,
        // because the bound alone is not enough: `string` is not a `T`.
        (Type::GenericParameter(_, _, Some(bound)), Type::Union(sup_members)) => {
            sup_members
                .iter()
                .any(|&member| is_subtype_inner(arena, sub, member, seen, binders))
                || is_subtype_inner(arena, *bound, sup, seen, binders)
        }

        // A type parameter stands for some type that satisfies its `extends`
        // bound, so whatever the bound is assignable to, the parameter is too:
        // `T extends string` can be returned as a string, passed where a string is
        // expected, or supplied as the argument for another `extends string`
        // parameter. Only this direction holds. An unconstrained parameter has
        // nothing to offer here and falls through to false, and nothing but
        // itself (identity, checked before we get here) or never/any is a
        // subtype *of* a parameter, since it could be instantiated as anything.
        (Type::GenericParameter(_, _, Some(bound)), _) => {
            is_subtype_inner(arena, *bound, sup, seen, binders)
        }

        (Type::StringLiteral(a), Type::StringLiteral(b)) => a == b,
        (Type::NumberLiteral(a), Type::NumberLiteral(b)) => a == b,
        (Type::BooleanLiteral(a), Type::BooleanLiteral(b)) => a == b,
        (Type::StringLiteral(_), Type::String) => true,
        (Type::NumberLiteral(_), Type::Number) => true,
        (Type::BooleanLiteral(_), Type::Boolean) => true,

        (Type::Never, _) => true,

        // undefined satisfies void (an implicit or bare `return;` is fine for a
        // void-returning function) but not the reverse, and nothing else is
        // interchangeable with void either; see Type::Void's own doc comment
        // on the leniency this deliberately doesn't implement.
        (Type::Undefined, Type::Void) => true,

        // These two arms are not symmetric on purpose. A union is a subtype of sup
        // only if every member is, since the value could be any of them and all
        // must qualify. sup accepts sub if any one member of sup does, since
        // matching one alternative is enough to satisfy an expected union.
        (Type::Union(sub_members), _) => sub_members
            .iter()
            .all(|&member| is_subtype_inner(arena, member, sup, seen, binders)),

        // To be an `A & B` a value has to be an `A` and a `B`: every member must accept
        // it. Ahead of the arms below so that an intersection against an intersection
        // is asked member by member.
        (_, Type::Intersection(sup_members)) => sup_members
            .iter()
            .all(|&member| is_subtype_inner(arena, sub, member, seen, binders)),

        // An `A & B` is a subtype of sup when either member already is: it can do
        // everything both can. Against a union both directions are tried, since the
        // union may accept the whole intersection (`A & B` into `A | C`) or one member
        // of it may fit inside the union (`A & B` into `(A | C)` because `A` does).
        (Type::Intersection(sub_members), Type::Union(sup_members)) => {
            sup_members
                .iter()
                .any(|&member| is_subtype_inner(arena, sub, member, seen, binders))
                || sub_members
                    .iter()
                    .any(|&member| is_subtype_inner(arena, member, sup, seen, binders))
        }

        (_, Type::Union(sup_members)) => sup_members
            .iter()
            .any(|&member| is_subtype_inner(arena, sub, member, seen, binders)),

        // Neither member of `{ a: number } & { b: string }` is assignable to `{ a:
        // number; b: string }`, but together they are. So an object target is also
        // asked of the members' properties taken as a whole.
        (Type::Intersection(sub_members), Type::Object(target)) => {
            sub_members
                .iter()
                .any(|&member| is_subtype_inner(arena, member, sup, seen, binders))
                || intersection_satisfies_object(arena, sub_members, target, seen, binders)
        }

        (Type::Intersection(sub_members), _) => sub_members
            .iter()
            .any(|&member| is_subtype_inner(arena, member, sup, seen, binders)),

        (Type::Function(a), Type::Function(b)) => function_is_subtype(arena, a, b, seen, binders),

        (Type::Array(a), Type::Array(b)) => is_subtype_inner(arena, *a, *b, seen, binders),

        (Type::Tuple(a), Type::Tuple(b)) => tuple_is_subtype(arena, a, b, seen, binders),

        // A tuple is an array of whatever its elements can be: `[string, number]` is a
        // `(string | number)[]`. The reverse does not hold, an array has no fixed length.
        (Type::Tuple(elements), Type::Array(target)) => elements
            .iter()
            .all(|e| is_subtype_inner(arena, e.type_id, *target, seen, binders)),

        (Type::Object(a), Type::Object(b)) => object_is_subtype(arena, a, b, seen, binders),

        _ => false,
    }
}

// Tuple against tuple. The source has to promise at least as many elements as the target
// requires, and no more than the target allows; every element it can supply then has to
// fit the target element at the same position (or the target's rest element past the
// last fixed one). `[string, number?]` fits `[string, number?]` and `[string, ...number[]]`
// but not `[string, number]`, which needs the second element to be there.
fn tuple_is_subtype(
    arena: &TypeArena,
    source: &[TupleElement],
    target: &[TupleElement],
    seen: &mut PairStack,
    binders: &[(TypeParameterId, TypeParameterId)],
) -> bool {
    let required = |t: &[TupleElement]| t.iter().filter(|e| !e.optional && !e.rest).count();
    let split = |t: &'_ [TupleElement]| -> (usize, Option<TupleElement>) {
        match t.last() {
            Some(last) if last.rest => (t.len() - 1, Some(*last)),
            _ => (t.len(), None),
        }
    };
    let (source_fixed, source_rest) = split(source);
    let (target_fixed, target_rest) = split(target);

    if required(source) < required(target) {
        return false;
    }
    // Without a rest element the target has a largest length the source must stay under,
    // and a source with a rest element has no largest length at all.
    if target_rest.is_none() && (source_rest.is_some() || source_fixed > target_fixed) {
        return false;
    }
    for (position, element) in source[..source_fixed].iter().enumerate() {
        let wanted = match target[..target_fixed].get(position) {
            Some(t) => t.type_id,
            None => match target_rest {
                Some(rest) => rest.type_id,
                None => return false,
            },
        };
        if !is_subtype_inner(arena, element.type_id, wanted, seen, binders) {
            return false;
        }
    }
    // A source rest element can stand at every target position from here on.
    if let Some(rest) = source_rest {
        for t in target[..target_fixed].iter().skip(source_fixed) {
            if !is_subtype_inner(arena, rest.type_id, t.type_id, seen, binders) {
                return false;
            }
        }
        if let Some(target_rest) = target_rest {
            return is_subtype_inner(arena, rest.type_id, target_rest.type_id, seen, binders);
        }
    }
    true
}

// Contravariant in parameters, covariant in return type: a function can stand in
// for another if it accepts everything the target promises to pass, or more, and
// returns something at least as specific as what the target promises to produce.
// Arity is checked separately from position-by-position compatibility, since
// optional and rest parameters mean two functions can have different declared
// parameter counts and still be safely interchangeable.
fn function_is_subtype(
    arena: &TypeArena,
    sub: &crate::types::FunctionType,
    sup: &crate::types::FunctionType,
    seen: &mut PairStack,
    binders: &[(TypeParameterId, TypeParameterId)],
) -> bool {
    function_is_subtype_with(arena, sub, sup, false, seen, binders)
}

// `bivariant_params` is tsc's rule for methods: a parameter position is compatible
// when the types are related in *either* direction, not only contravariantly. It
// applies to the parameters only; the return type stays covariant.
fn function_is_subtype_with(
    arena: &TypeArena,
    sub: &crate::types::FunctionType,
    sup: &crate::types::FunctionType,
    bivariant_params: bool,
    seen: &mut PairStack,
    binders: &[(TypeParameterId, TypeParameterId)],
) -> bool {
    // A generic signature is universally quantified. Compare corresponding binders
    // under one shared alpha-renaming environment instead of treating their parameter
    // IDs as unrelated free variables. A generic/non-generic pair is not equivalent.
    if sub.type_params.len() != sup.type_params.len() {
        return false;
    }
    let mut active_binders = binders.to_vec();
    for (source, target) in sub.type_params.iter().zip(&sup.type_params) {
        active_binders.push((source.id, target.id));
    }
    for (source, target) in sub.type_params.iter().zip(&sup.type_params) {
        match (source.constraint, target.constraint) {
            // The target's constraint describes the values callers may instantiate.
            // That set must fit inside the source's allowed set. An unconstrained
            // source can accept a constrained target, but not the reverse.
            (None, _) => {}
            (Some(_), None) => return false,
            (Some(source_bound), Some(target_bound)) => {
                if !is_subtype_inner(arena, target_bound, source_bound, seen, &active_binders) {
                    return false;
                }
            }
        }
    }
    // sub cannot require more arguments than callers of sup are guaranteed to
    // supply. It is free to require fewer; its extra optional or rest slots simply
    // never get filled by such a caller.
    if required_param_count(&sub.params) > required_param_count(&sup.params) {
        return false;
    }

    let checked_positions = sub.params.len().max(sup.params.len());
    for position in 0..checked_positions {
        if let (Some(sub_param), Some(sup_param)) = (
            param_type_at(arena, &sub.params, position),
            param_type_at(arena, &sup.params, position),
        ) && !is_subtype_inner(arena, sup_param, sub_param, seen, &active_binders)
            && !(bivariant_params
                && is_subtype_inner(arena, sub_param, sup_param, seen, &active_binders))
        {
            return false;
        }
    }

    // TypeScript permits a value-returning function wherever a void-returning
    // callback is expected: the caller promises to ignore the result. This is a
    // function-compatibility rule, not a general rule that `number` is a subtype
    // of `void`, so keep it here rather than weakening the primitive relation.
    matches!(arena.get(sup.return_type), Type::Void)
        || is_subtype_inner(
            arena,
            sub.return_type,
            sup.return_type,
            seen,
            &active_binders,
        )
}

fn required_param_count(params: &[Param]) -> usize {
    params.iter().filter(|p| !p.optional && !p.rest).count()
}

// The type expected at a given argument position. A rest parameter absorbs every
// position past the end of the declared list, and its own declared type is the
// array type, such as number[], not the per-argument type, so it gets unwrapped to
// the element type here.
pub(crate) fn param_type_at(
    arena: &TypeArena,
    params: &[Param],
    position: usize,
) -> Option<TypeId> {
    let param = match params.get(position) {
        Some(param) => param,
        None => params.last().filter(|p| p.rest)?,
    };
    Some(if param.rest {
        match arena.get(param.type_id) {
            Type::Array(element) => *element,
            // `...args: [string, number?]`: the argument at this position is the tuple
            // element at the same offset into the rest, or its rest element past the end.
            Type::Tuple(elements) => {
                let offset = position.saturating_sub(params.len() - 1);
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

// A merge-join over both property lists, which relies on ObjectType.properties
// always being kept sorted by name. This makes the check a single linear pass
// instead of a lookup per property. sub's extra properties are simply skipped by
// the peekable iterator, which is where width subtyping (sub may have more fields
// than sup) falls out for free, and any sup property sub never reaches is only
// acceptable if sup itself marks that property optional.
fn object_is_subtype(
    arena: &TypeArena,
    sub: &ObjectType,
    sup: &ObjectType,
    seen: &mut PairStack,
    binders: &[(TypeParameterId, TypeParameterId)],
) -> bool {
    // The merge-join below silently gives wrong answers on unsorted input, so an
    // unsorted ObjectType reaching here is a construction bug elsewhere, not
    // something to tolerate. Checked in debug builds (which is what the tests run).
    debug_assert!(
        is_sorted_by_name(&sub.properties) && is_sorted_by_name(&sup.properties),
        "ObjectType properties must be sorted by name; build them with ObjectType::new"
    );

    let mut sub_properties = sub.properties.iter().peekable();

    'sup_properties: for sup_property in sup.properties.iter() {
        while let Some(sub_property) = sub_properties.peek() {
            match sub_property.name.cmp(&sup_property.name) {
                Ordering::Less => {
                    sub_properties.next();
                }
                Ordering::Equal => {
                    let Some(sub_property) = sub_properties.next() else {
                        break;
                    };
                    if sub_property.optional && !sup_property.optional {
                        return false;
                    }
                    if !property_is_subtype(
                        arena,
                        sub_property.type_id,
                        sup_property,
                        seen,
                        binders,
                    ) {
                        return false;
                    }
                    continue 'sup_properties;
                }
                Ordering::Greater => break,
            }
        }
        if !sup_property.optional {
            return false;
        }
    }

    true
}

// Whether the members of an intersection, between them, give the target object everything
// it asks for. For each property of the target some member must provide one whose type
// fits, and it must be required if the target requires it: it is required in the
// intersection when any member requires it. A property no member has is fine only when
// the target made it optional.
fn intersection_satisfies_object(
    arena: &TypeArena,
    members: &[TypeId],
    target: &ObjectType,
    seen: &mut PairStack,
    binders: &[(TypeParameterId, TypeParameterId)],
) -> bool {
    target.properties.iter().all(|wanted| {
        let provided: Vec<&crate::types::PropertyEntry> = members
            .iter()
            .filter_map(|&member| match arena.get(member) {
                Type::Object(object) => object.properties.iter().find(|p| p.name == wanted.name),
                _ => None,
            })
            .collect();
        if provided.is_empty() {
            return wanted.optional;
        }
        let required_here = provided.iter().any(|property| !property.optional);
        (wanted.optional || required_here)
            && provided
                .iter()
                .any(|property| property_is_subtype(arena, property.type_id, wanted, seen, binders))
    })
}

// A property's type against the target property. When the target was declared
// with method syntax and both sides are functions, tsc's method rule applies
// (bivariant parameters); anything else is ordinary subtyping. The rule follows
// the target's declaration, so a function-typed property (`f: (a: A) => R`) stays
// strictly contravariant.
fn property_is_subtype(
    arena: &TypeArena,
    sub_type: TypeId,
    sup_property: &crate::types::PropertyEntry,
    seen: &mut PairStack,
    binders: &[(TypeParameterId, TypeParameterId)],
) -> bool {
    if sup_property.is_method
        && let (Type::Function(sub_function), Type::Function(sup_function)) =
            (arena.get(sub_type), arena.get(sup_property.type_id))
    {
        return function_is_subtype_with(arena, sub_function, sup_function, true, seen, binders);
    }
    is_subtype_inner(arena, sub_type, sup_property.type_id, seen, binders)
}

// The same property rule `object_is_subtype` applies, for a caller that has already
// found a mismatch and needs to know which property it is (see explain.rs). Kept next
// to `property_is_subtype` so the two cannot drift apart.
pub(crate) fn property_relates(
    arena: &TypeArena,
    sub_type: TypeId,
    sup_property: &crate::types::PropertyEntry,
) -> bool {
    property_is_subtype(arena, sub_type, sup_property, &mut PairStack::new(), &[])
}

fn is_sorted_by_name(properties: &[crate::types::PropertyEntry]) -> bool {
    properties.is_sorted_by(|a, b| a.name <= b.name)
}

// Any and Error both act as escape hatches, compatible with everything in both
// directions, but for different reasons. Any is TypeScript's own opt-out from
// checking. Error is this checker's internal sentinel for an expression that
// already failed to type-check; treating it as universally compatible stops one
// mistake from cascading into a wall of unrelated-looking follow-on errors.
// The answers that need no structural walk at all: the same id, or an escape-hatch
// type (Any, or Error standing in for an already-reported failure) on either side.
//
// Lives here, and is shared with SemanticQueries, so the cache's fast path and the
// real relation cannot drift apart. If the cache bypassed on a rule that this
// function did not also apply, a cached run and an uncached run could disagree, and
// that class of bug only shows up as flaky diagnostics, never as a crash.
pub(crate) fn is_trivial_subtype(arena: &TypeArena, sub: TypeId, sup: TypeId) -> bool {
    sub == sup || is_universally_compatible(arena, sub) || is_universally_compatible(arena, sup)
}

fn is_universally_compatible(arena: &TypeArena, id: TypeId) -> bool {
    matches!(arena.get(id), Type::Any | Type::Error)
}

// Whether no value can belong to both types. It answers true only when that is certain
// and false whenever it is not.
//
// The bias is deliberate. The reason to ask is narrowing: `x === "a"` may discard the
// union members that cannot equal "a". Wrongly saying "disjoint" throws away a member
// that could still be present and produces a false error later, while wrongly saying
// "overlapping" only keeps a member a smarter check would have dropped. So every case
// this cannot decide, objects, arrays, functions, type parameters, `unknown`, says false.
// `{ length: number }` overlaps `string`, and an empty object overlaps everything that
// is not null, so a shape alone never proves two object-like types disjoint.
pub(crate) fn is_disjoint(arena: &TypeArena, a: TypeId, b: TypeId) -> bool {
    // A type with no values shares none with anything, itself included.
    if a == b {
        return matches!(arena.get(a), Type::Never);
    }
    match (arena.get(a), arena.get(b)) {
        (Type::Never, _) | (_, Type::Never) => return true,
        (Type::Any | Type::Unknown | Type::Error | Type::GenericParameter(..), _)
        | (_, Type::Any | Type::Unknown | Type::Error | Type::GenericParameter(..)) => {
            return false;
        }
        _ => {}
    }

    // A union is disjoint from something only if every member is. Members are never
    // unions themselves (alloc_union flattens), so this recursion is shallow.
    if let Type::Union(members) = arena.get(a) {
        return members.iter().all(|&member| is_disjoint(arena, member, b));
    }
    if let Type::Union(members) = arena.get(b) {
        return members.iter().all(|&member| is_disjoint(arena, a, member));
    }

    let (left, right) = (arena.get(a), arena.get(b));
    match (primitive_domain(left), primitive_domain(right)) {
        (Some(l), Some(r)) if l != r => true,
        // Same domain: only two literals with different values exclude each other.
        // `string` and "a" overlap, and so do `number` and 1.
        (Some(_), Some(_)) => match (left, right) {
            (Type::StringLiteral(x), Type::StringLiteral(y)) => x != y,
            (Type::NumberLiteral(x), Type::NumberLiteral(y)) => x != y,
            (Type::BooleanLiteral(x), Type::BooleanLiteral(y)) => x != y,
            _ => false,
        },
        _ => false,
    }
}

#[derive(PartialEq, Clone, Copy)]
pub(crate) enum PrimitiveDomain {
    String,
    Number,
    Boolean,
    Null,
    Undefined,
}

// Which kind of primitive value a type holds, when it holds only one kind. `void` shares
// a domain with `undefined` because undefined is assignable to void, so the two overlap.
pub(crate) fn primitive_domain(ty: &Type) -> Option<PrimitiveDomain> {
    match ty {
        Type::String | Type::StringLiteral(_) => Some(PrimitiveDomain::String),
        Type::Number | Type::NumberLiteral(_) => Some(PrimitiveDomain::Number),
        Type::Boolean | Type::BooleanLiteral(_) => Some(PrimitiveDomain::Boolean),
        Type::Null => Some(PrimitiveDomain::Null),
        Type::Undefined | Type::Void => Some(PrimitiveDomain::Undefined),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn literal(arena: &mut TypeArena, value: &str) -> TypeId {
        arena.alloc(Type::StringLiteral(value.to_string()))
    }

    #[test]
    fn different_primitive_kinds_are_disjoint() {
        let arena = TypeArena::new();
        assert!(is_disjoint(&arena, arena.string(), arena.number()));
        assert!(is_disjoint(&arena, arena.number(), arena.null()));
        assert!(is_disjoint(&arena, arena.boolean(), arena.undefined()));
        assert!(is_disjoint(&arena, arena.null(), arena.undefined()));
    }

    #[test]
    fn a_type_overlaps_itself_and_its_own_literals() {
        let mut arena = TypeArena::new();
        let a = literal(&mut arena, "a");
        assert!(!is_disjoint(&arena, arena.string(), arena.string()));
        assert!(!is_disjoint(&arena, arena.string(), a));
        assert!(!is_disjoint(&arena, a, arena.string()));
    }

    #[test]
    fn literals_with_different_values_are_disjoint_and_equal_ones_are_not() {
        let mut arena = TypeArena::new();
        let a = literal(&mut arena, "a");
        let b = literal(&mut arena, "b");
        assert!(is_disjoint(&arena, a, b));
        assert!(!is_disjoint(&arena, a, a));
    }

    #[test]
    fn undefined_and_void_overlap() {
        let arena = TypeArena::new();
        assert!(!is_disjoint(&arena, arena.undefined(), arena.void()));
    }

    #[test]
    fn escape_hatches_and_unknown_are_never_disjoint() {
        let arena = TypeArena::new();
        for open in [arena.any(), arena.unknown(), arena.error()] {
            assert!(!is_disjoint(&arena, open, arena.number()));
            assert!(!is_disjoint(&arena, arena.string(), open));
        }
    }

    #[test]
    fn object_like_types_are_not_proven_disjoint_from_primitives() {
        let mut arena = TypeArena::new();
        let number = arena.number();
        let object = arena.alloc(Type::Object(ObjectType::new(vec![
            crate::types::PropertyEntry {
                name: "length".into(),
                type_id: number,
                optional: false,
                is_method: false,
            },
        ])));
        let array = arena.alloc(Type::Array(number));

        assert!(!is_disjoint(&arena, object, arena.string()));
        assert!(!is_disjoint(&arena, array, arena.string()));
        assert!(!is_disjoint(&arena, object, array));
    }

    #[test]
    fn never_is_disjoint_from_everything() {
        let arena = TypeArena::new();
        assert!(is_disjoint(&arena, arena.never(), arena.number()));
        assert!(is_disjoint(&arena, arena.string(), arena.never()));
        assert!(is_disjoint(&arena, arena.never(), arena.never()));
    }

    #[test]
    fn a_union_is_disjoint_only_when_every_member_is() {
        let mut arena = TypeArena::new();
        let (number, string) = (arena.number(), arena.string());
        let both = arena.alloc_union(vec![number, string]);
        let a = literal(&mut arena, "a");

        assert!(is_disjoint(&arena, both, arena.null()));
        assert!(!is_disjoint(&arena, both, a));
        assert!(!is_disjoint(&arena, arena.string(), both));
    }

    use super::*;

    #[test]
    fn undefined_satisfies_void() {
        let arena = TypeArena::new();
        assert!(is_subtype(&arena, arena.undefined(), arena.void()));
    }

    #[test]
    fn void_does_not_satisfy_undefined() {
        let arena = TypeArena::new();
        assert!(!is_subtype(&arena, arena.void(), arena.undefined()));
    }

    #[test]
    fn nothing_else_satisfies_void() {
        let arena = TypeArena::new();
        assert!(!is_subtype(&arena, arena.number(), arena.void()));
        assert!(!is_subtype(&arena, arena.string(), arena.void()));
    }

    #[test]
    fn void_does_not_satisfy_other_types() {
        let arena = TypeArena::new();
        assert!(!is_subtype(&arena, arena.void(), arena.number()));
    }

    #[test]
    fn never_satisfies_void_like_it_satisfies_everything() {
        let arena = TypeArena::new();
        assert!(is_subtype(&arena, arena.never(), arena.void()));
    }

    #[test]
    fn object_built_in_reverse_name_order_is_still_a_subtype() {
        let mut arena = TypeArena::new();
        let (number, string) = (arena.number(), arena.string());
        // Declared z-before-a, the way enum members are in source order.
        let sub = arena.alloc(object_type(&[("z", string, false), ("a", number, false)]));
        let sup = arena.alloc(object_type(&[("a", number, false), ("z", string, false)]));

        assert!(is_subtype(&arena, sub, sup));
        assert!(is_subtype(&arena, sup, sub));
    }

    #[test]
    fn primitive_is_subtype_of_itself() {
        let arena = TypeArena::new();
        assert!(is_subtype(&arena, arena.number(), arena.number()));
    }

    #[test]
    fn number_is_not_subtype_of_string() {
        let arena = TypeArena::new();
        assert!(!is_subtype(&arena, arena.number(), arena.string()));
    }

    #[test]
    fn any_is_compatible_in_both_directions() {
        let arena = TypeArena::new();
        assert!(is_subtype(&arena, arena.any(), arena.string()));
        assert!(is_subtype(&arena, arena.string(), arena.any()));
    }

    #[test]
    fn error_sentinel_does_not_cascade_into_a_mismatch() {
        let arena = TypeArena::new();
        assert!(is_subtype(&arena, arena.error(), arena.number()));
        assert!(is_subtype(&arena, arena.number(), arena.error()));
    }

    #[test]
    fn everything_is_subtype_of_unknown() {
        let arena = TypeArena::new();
        assert!(is_subtype(&arena, arena.string(), arena.unknown()));
    }

    #[test]
    fn unknown_is_not_assignable_to_a_concrete_type() {
        let arena = TypeArena::new();
        assert!(!is_subtype(&arena, arena.unknown(), arena.string()));
    }

    #[test]
    fn generic_function_signatures_are_compared_alpha_equivalently() {
        use crate::types::{FunctionType, FunctionTypeParam, TypeParameterId};

        let mut arena = TypeArena::new();
        let source_id = TypeParameterId::new(100, 0);
        let target_id = TypeParameterId::new(200, 0);
        let source_t = arena.alloc(Type::GenericParameter(source_id, "T".into(), None));
        let target_u = arena.alloc(Type::GenericParameter(target_id, "U".into(), None));
        let source = arena.alloc(Type::Function(FunctionType {
            params: vec![Param::required(source_t)],
            return_type: source_t,
            type_params: vec![FunctionTypeParam {
                id: source_id,
                name: "T".into(),
                constraint: None,
                default: None,
            }],
            is_untyped: false,
        }));
        let target = arena.alloc(Type::Function(FunctionType {
            params: vec![Param::required(target_u)],
            return_type: target_u,
            type_params: vec![FunctionTypeParam {
                id: target_id,
                name: "U".into(),
                constraint: None,
                default: None,
            }],
            is_untyped: false,
        }));

        assert!(is_subtype(&arena, source, target));
        assert!(is_subtype(&arena, target, source));
    }

    #[test]
    fn generic_signature_constraints_are_compared_in_the_target_to_source_direction() {
        use crate::types::{FunctionType, FunctionTypeParam, TypeParameterId};

        let mut arena = TypeArena::new();
        let source_id = TypeParameterId::new(100, 0);
        let target_id = TypeParameterId::new(200, 0);
        let string = arena.string();
        let only_foo = arena.alloc(Type::StringLiteral("foo".into()));
        let source_t = arena.alloc(Type::GenericParameter(source_id, "T".into(), Some(string)));
        let target_u = arena.alloc(Type::GenericParameter(
            target_id,
            "U".into(),
            Some(only_foo),
        ));
        let source = arena.alloc(Type::Function(FunctionType {
            params: vec![Param::required(source_t)],
            return_type: source_t,
            type_params: vec![FunctionTypeParam {
                id: source_id,
                name: "T".into(),
                constraint: Some(string),
                default: None,
            }],
            is_untyped: false,
        }));
        let target = arena.alloc(Type::Function(FunctionType {
            params: vec![Param::required(target_u)],
            return_type: target_u,
            type_params: vec![FunctionTypeParam {
                id: target_id,
                name: "U".into(),
                constraint: Some(only_foo),
                default: None,
            }],
            is_untyped: false,
        }));

        assert!(is_subtype(&arena, source, target));
        assert!(!is_subtype(&arena, target, source));
    }

    #[test]
    fn generic_signature_does_not_satisfy_a_universal_contract_with_one_concrete_signature() {
        use crate::types::{FunctionType, FunctionTypeParam, TypeParameterId};

        let mut arena = TypeArena::new();
        let id = TypeParameterId::new(100, 0);
        let t = arena.alloc(Type::GenericParameter(id, "T".into(), None));
        let generic = arena.alloc(Type::Function(FunctionType {
            params: vec![Param::required(t)],
            return_type: t,
            type_params: vec![FunctionTypeParam {
                id,
                name: "T".into(),
                constraint: None,
                default: None,
            }],
            is_untyped: false,
        }));
        let concrete = arena.alloc(Type::Function(FunctionType {
            params: vec![Param::required(arena.string())],
            return_type: arena.string(),
            type_params: Vec::new(),
            is_untyped: false,
        }));

        assert!(!is_subtype(&arena, concrete, generic));
    }

    #[test]
    fn function_subtyping_is_contravariant_in_params_covariant_in_return() {
        use crate::types::FunctionType;

        let mut arena = TypeArena::new();

        let sub = arena.alloc(Type::Function(FunctionType {
            params: vec![Param::required(arena.unknown())],
            return_type: arena.number(),
            type_params: Vec::new(),
            is_untyped: false,
        }));

        let sup = arena.alloc(Type::Function(FunctionType {
            params: vec![Param::required(arena.string())],
            return_type: arena.unknown(),
            type_params: Vec::new(),
            is_untyped: false,
        }));

        assert!(is_subtype(&arena, sub, sup));
        assert!(!is_subtype(&arena, sup, sub));
    }

    #[test]
    fn value_returning_function_is_compatible_with_void_callback() {
        use crate::types::FunctionType;

        let mut arena = TypeArena::new();
        let returns_number = arena.alloc(Type::Function(FunctionType {
            params: Vec::new(),
            return_type: arena.number(),
            type_params: Vec::new(),
            is_untyped: false,
        }));
        let returns_void = arena.alloc(Type::Function(FunctionType {
            params: Vec::new(),
            return_type: arena.void(),
            type_params: Vec::new(),
            is_untyped: false,
        }));

        assert!(is_subtype(&arena, returns_number, returns_void));
        // The exception is intentionally one-way: a void callback cannot promise
        // to return a number to a caller that expects to consume one.
        assert!(!is_subtype(&arena, returns_void, returns_number));
        // This must not change ordinary value assignability.
        assert!(!is_subtype(&arena, arena.number(), arena.void()));
    }

    #[test]
    fn optional_param_lets_sub_accept_fewer_required_args() {
        use crate::types::FunctionType;

        let mut arena = TypeArena::new();

        let sub = arena.alloc(Type::Function(FunctionType {
            params: vec![
                Param::required(arena.number()),
                Param {
                    type_id: arena.number(),
                    optional: true,
                    rest: false,
                    name: None,
                },
            ],
            return_type: arena.undefined(),
            type_params: Vec::new(),
            is_untyped: false,
        }));

        let sup = arena.alloc(Type::Function(FunctionType {
            params: vec![Param::required(arena.number())],
            return_type: arena.undefined(),
            type_params: Vec::new(),
            is_untyped: false,
        }));

        assert!(is_subtype(&arena, sub, sup));
    }

    #[test]
    fn function_requiring_more_args_than_target_guarantees_is_not_a_subtype() {
        use crate::types::FunctionType;

        let mut arena = TypeArena::new();

        let needs_two = arena.alloc(Type::Function(FunctionType {
            params: vec![
                Param::required(arena.number()),
                Param::required(arena.number()),
            ],
            return_type: arena.undefined(),
            type_params: Vec::new(),
            is_untyped: false,
        }));

        let needs_one = arena.alloc(Type::Function(FunctionType {
            params: vec![Param::required(arena.number())],
            return_type: arena.undefined(),
            type_params: Vec::new(),
            is_untyped: false,
        }));

        assert!(!is_subtype(&arena, needs_two, needs_one));

        assert!(is_subtype(&arena, needs_one, needs_two));
    }

    #[test]
    fn rest_param_lets_sub_accept_any_number_of_trailing_args() {
        use crate::types::FunctionType;

        let mut arena = TypeArena::new();
        let number_array = arena.alloc(Type::Array(arena.number()));

        let sub = arena.alloc(Type::Function(FunctionType {
            params: vec![Param {
                type_id: number_array,
                optional: false,
                rest: true,
                name: None,
            }],
            return_type: arena.undefined(),
            type_params: Vec::new(),
            is_untyped: false,
        }));

        let sup = arena.alloc(Type::Function(FunctionType {
            params: vec![
                Param::required(arena.number()),
                Param::required(arena.number()),
            ],
            return_type: arena.undefined(),
            type_params: Vec::new(),
            is_untyped: false,
        }));

        assert!(is_subtype(&arena, sub, sup));
    }

    fn object_type(props: &[(&str, TypeId, bool)]) -> Type {
        use crate::types::{ObjectType, PropertyEntry};

        let properties: Vec<PropertyEntry> = props
            .iter()
            .map(|&(name, type_id, optional)| PropertyEntry {
                name: name.into(),
                type_id,
                optional,
                is_method: false,
            })
            .collect();
        Type::Object(ObjectType::new(properties))
    }

    #[test]
    fn object_with_matching_properties_is_subtype() {
        let mut arena = TypeArena::new();
        let sub = arena.alloc(object_type(&[
            ("name", arena.string(), false),
            ("age", arena.number(), false),
        ]));
        let sup = arena.alloc(object_type(&[
            ("name", arena.string(), false),
            ("age", arena.number(), false),
        ]));
        assert!(is_subtype(&arena, sub, sup));
    }

    #[test]
    fn object_with_extra_property_is_still_subtype() {
        let mut arena = TypeArena::new();
        let sub = arena.alloc(object_type(&[
            ("name", arena.string(), false),
            ("age", arena.number(), false),
            ("id", arena.number(), false),
        ]));
        let sup = arena.alloc(object_type(&[("name", arena.string(), false)]));
        assert!(is_subtype(&arena, sub, sup));
    }

    #[test]
    fn object_missing_a_required_property_is_not_subtype() {
        let mut arena = TypeArena::new();
        let sub = arena.alloc(object_type(&[("name", arena.string(), false)]));
        let sup = arena.alloc(object_type(&[
            ("name", arena.string(), false),
            ("age", arena.number(), false),
        ]));
        assert!(!is_subtype(&arena, sub, sup));
    }

    #[test]
    fn object_missing_an_optional_property_is_still_subtype() {
        let mut arena = TypeArena::new();
        let sub = arena.alloc(object_type(&[("name", arena.string(), false)]));
        let sup = arena.alloc(object_type(&[
            ("name", arena.string(), false),
            ("age", arena.number(), true),
        ]));
        assert!(is_subtype(&arena, sub, sup));
    }

    #[test]
    fn optional_property_is_not_subtype_of_required_property() {
        let mut arena = TypeArena::new();
        let sub = arena.alloc(object_type(&[("value", arena.number(), true)]));
        let sup = arena.alloc(object_type(&[("value", arena.number(), false)]));
        assert!(!is_subtype(&arena, sub, sup));
    }

    #[test]
    fn object_with_mismatched_property_type_is_not_subtype() {
        let mut arena = TypeArena::new();
        let sub = arena.alloc(object_type(&[("age", arena.string(), false)]));
        let sup = arena.alloc(object_type(&[("age", arena.number(), false)]));
        assert!(!is_subtype(&arena, sub, sup));
    }

    #[test]
    fn array_element_type_is_covariant() {
        let mut arena = TypeArena::new();
        let sub = arena.alloc(Type::Array(arena.number()));
        let sup = arena.alloc(Type::Array(arena.unknown()));
        assert!(is_subtype(&arena, sub, sup));
        assert!(!is_subtype(&arena, sup, sub));
    }

    #[test]
    fn array_of_mismatched_element_types_is_not_subtype() {
        let mut arena = TypeArena::new();
        let sub = arena.alloc(Type::Array(arena.number()));
        let sup = arena.alloc(Type::Array(arena.string()));
        assert!(!is_subtype(&arena, sub, sup));
    }

    #[test]
    fn concrete_type_is_subtype_of_a_union_containing_it() {
        let mut arena = TypeArena::new();
        let union = arena.alloc(Type::Union(vec![arena.number(), arena.string()]));
        assert!(is_subtype(&arena, arena.number(), union));
    }

    #[test]
    fn concrete_type_is_not_subtype_of_a_union_missing_it() {
        let mut arena = TypeArena::new();
        let union = arena.alloc(Type::Union(vec![arena.number(), arena.string()]));
        assert!(!is_subtype(&arena, arena.boolean(), union));
    }

    #[test]
    fn union_is_subtype_of_sup_only_if_every_member_is() {
        let mut arena = TypeArena::new();
        let sub_union = arena.alloc(Type::Union(vec![arena.number(), arena.string()]));

        assert!(is_subtype(&arena, sub_union, arena.unknown()));

        assert!(!is_subtype(&arena, sub_union, arena.number()));
    }

    #[test]
    fn literal_widens_to_its_base_primitive() {
        let mut arena = TypeArena::new();
        let five = arena.alloc(Type::NumberLiteral(5.0));
        assert!(is_subtype(&arena, five, arena.number()));
        assert!(!is_subtype(&arena, arena.number(), five));
    }

    #[test]
    fn equal_literals_are_subtypes_of_each_other() {
        let mut arena = TypeArena::new();
        let a = arena.alloc(Type::StringLiteral("hi".to_string()));
        let b = arena.alloc(Type::StringLiteral("hi".to_string()));
        assert!(is_subtype(&arena, a, b));
    }

    #[test]
    fn different_literals_are_not_subtypes_of_each_other() {
        let mut arena = TypeArena::new();
        let a = arena.alloc(Type::StringLiteral("hi".to_string()));
        let b = arena.alloc(Type::StringLiteral("bye".to_string()));
        assert!(!is_subtype(&arena, a, b));
    }

    #[test]
    fn never_is_subtype_of_everything() {
        let arena = TypeArena::new();
        assert!(is_subtype(&arena, arena.never(), arena.string()));
        assert!(is_subtype(&arena, arena.never(), arena.any()));
    }

    #[test]
    fn nothing_but_never_is_a_subtype_of_never() {
        let arena = TypeArena::new();
        assert!(!is_subtype(&arena, arena.string(), arena.never()));
    }

    #[test]
    fn union_construction_flattens_and_dedupes() {
        let mut arena = TypeArena::new();
        let inner = arena.alloc(Type::Union(vec![arena.string(), arena.number()]));
        let outer = arena.alloc_union(vec![inner, arena.number(), arena.boolean()]);
        let members = match arena.get(outer) {
            Type::Union(members) => members,
            _ => panic!("expected a union"),
        };
        assert_eq!(members.len(), 3);
    }

    #[test]
    fn union_dedupes_equal_literals_even_with_different_type_ids() {
        let mut arena = TypeArena::new();
        // alloc_fresh, not alloc: alloc() reuses one id for identical literals, and
        // this test is specifically about two equal literals in different slots.
        let a1 = arena.alloc_fresh(Type::StringLiteral("a".to_string()));
        let a2 = arena.alloc_fresh(Type::StringLiteral("a".to_string()));
        assert_ne!(
            a1, a2,
            "test setup: these must be genuinely different TypeIds"
        );

        let result = arena.alloc_union(vec![a1, a2]);

        assert!(
            result == a1 || result == a2,
            "expected exactly one of the two equal literals to survive"
        );
        assert_eq!(
            arena.get(result),
            &Type::StringLiteral("a".to_string()),
            "collapsed result should still be the string literal 'a'"
        );
    }

    #[test]
    fn union_of_never_and_one_other_type_collapses_to_that_type() {
        let mut arena = TypeArena::new();
        let never = arena.never();
        let result = arena.alloc_union(vec![never, arena.string()]);
        assert_eq!(result, arena.string());
    }

    #[test]
    fn union_of_only_never_collapses_to_never() {
        let mut arena = TypeArena::new();
        let result = arena.alloc_union(vec![arena.never()]);
        assert_eq!(result, arena.never());
    }

    #[test]
    fn constrained_type_parameter_is_a_subtype_of_its_bound() {
        use crate::types::TypeParameterId;

        let mut arena = TypeArena::new();
        let (number, string) = (arena.number(), arena.string());
        let bounded = arena.alloc(Type::GenericParameter(
            TypeParameterId::new(0, 0),
            "T".to_string(),
            Some(string),
        ));
        let unbounded = arena.alloc(Type::GenericParameter(
            TypeParameterId::new(1, 0),
            "U".to_string(),
            None,
        ));
        let string_or_number = arena.alloc_union(vec![string, number]);

        assert!(is_subtype(&arena, bounded, string));
        assert!(is_subtype(&arena, bounded, string_or_number));
        assert!(!is_subtype(&arena, bounded, number));
        assert!(!is_subtype(&arena, string, bounded));
        assert!(!is_subtype(&arena, unbounded, string));
    }
}

// Assignability to and from an intersection (LLD 1.13, "Relations"): a target needs every
// member, a source needs some member or, for an object target, members that together
// have what it asks for.
#[cfg(test)]
mod intersection_relation_tests {
    use super::*;
    use crate::types::{FunctionType, ObjectType, Param, PropertyEntry, TypeParameterId};

    fn object(arena: &mut TypeArena, properties: &[(&str, TypeId, bool)]) -> TypeId {
        let entries = properties
            .iter()
            .map(|&(name, type_id, optional)| PropertyEntry {
                name: name.into(),
                type_id,
                optional,
                is_method: false,
            })
            .collect();
        arena.alloc(Type::Object(ObjectType::new(entries)))
    }

    fn function(arena: &mut TypeArena, parameter: TypeId, returns: TypeId) -> TypeId {
        arena.alloc(Type::Function(FunctionType {
            params: vec![Param {
                type_id: parameter,
                optional: false,
                rest: false,
                name: None,
            }],
            return_type: returns,
            type_params: Vec::new(),
            is_untyped: false,
        }))
    }

    fn both(arena: &mut TypeArena, parts: &[TypeId]) -> TypeId {
        arena
            .alloc_intersection(parts.to_vec())
            .expect("this intersection is small enough to build")
    }

    // `{ a: number }`, `{ b: string }` and their intersection.
    fn pair(arena: &mut TypeArena) -> (TypeId, TypeId, TypeId) {
        let (number, string) = (arena.number(), arena.string());
        let a = object(arena, &[("a", number, false)]);
        let b = object(arena, &[("b", string, false)]);
        let ab = both(arena, &[a, b]);
        (a, b, ab)
    }

    #[test]
    fn a_source_has_to_satisfy_every_member_of_an_intersection_target() {
        let mut arena = TypeArena::new();
        let (number, string) = (arena.number(), arena.string());
        let (a, _, ab) = pair(&mut arena);
        let has_both = object(&mut arena, &[("a", number, false), ("b", string, false)]);

        assert!(is_subtype(&arena, has_both, ab));
        assert!(!is_subtype(&arena, a, ab), "it lacks `b`");
    }

    #[test]
    fn an_intersection_is_a_subtype_of_each_of_its_members_and_of_nothing_it_lacks() {
        let mut arena = TypeArena::new();
        let boolean = arena.boolean();
        let (a, b, ab) = pair(&mut arena);
        let c = object(&mut arena, &[("c", boolean, false)]);

        assert!(is_subtype(&arena, ab, a));
        assert!(is_subtype(&arena, ab, b));
        assert!(!is_subtype(&arena, ab, c));
    }

    // Neither `{ a }` nor `{ b }` is assignable to `{ a; b }`, but their intersection is.
    #[test]
    fn the_members_together_satisfy_an_object_none_of_them_satisfies_alone() {
        let mut arena = TypeArena::new();
        let (number, string, boolean) = (arena.number(), arena.string(), arena.boolean());
        let (a, b, ab) = pair(&mut arena);
        let wants_both = object(&mut arena, &[("a", number, false), ("b", string, false)]);
        let wrong_type = object(&mut arena, &[("a", number, false), ("b", number, false)]);
        let wants_more = object(&mut arena, &[("a", number, false), ("c", boolean, false)]);
        let optional_extra = object(&mut arena, &[("a", number, false), ("c", boolean, true)]);

        assert!(!is_subtype(&arena, a, wants_both));
        assert!(!is_subtype(&arena, b, wants_both));
        assert!(is_subtype(&arena, ab, wants_both));
        assert!(!is_subtype(&arena, ab, wrong_type));
        assert!(!is_subtype(&arena, ab, wants_more));
        assert!(is_subtype(&arena, ab, optional_extra));
    }

    // A property is required in the intersection when any member requires it.
    #[test]
    fn a_property_is_required_if_any_member_requires_it() {
        let mut arena = TypeArena::new();
        let number = arena.number();
        let optional = object(&mut arena, &[("a", number, true)]);
        let required = object(&mut arena, &[("a", number, false)]);
        let wants_a = object(&mut arena, &[("a", number, false)]);

        let only_optional = both(&mut arena, &[optional, optional]);
        assert!(!is_subtype(&arena, only_optional, wants_a));
        let mixed = both(&mut arena, &[optional, required]);
        assert!(is_subtype(&arena, mixed, wants_a));
    }

    #[test]
    fn an_intersection_into_a_union_goes_by_the_whole_or_by_a_member() {
        let mut arena = TypeArena::new();
        let (number, boolean) = (arena.number(), arena.boolean());
        let (a, b, ab) = pair(&mut arena);
        let c = object(&mut arena, &[("c", boolean, false)]);
        let d = object(&mut arena, &[("d", number, false)]);

        let a_or_c = arena.alloc_union(vec![a, c]);
        let c_or_d = arena.alloc_union(vec![c, d]);
        assert!(is_subtype(&arena, ab, a_or_c), "because it is an `a`");
        let b_or_c = arena.alloc_union(vec![b, c]);
        assert!(is_subtype(&arena, b, b_or_c));
        assert!(!is_subtype(&arena, ab, c_or_d));
    }

    #[test]
    fn an_intersection_that_can_have_no_value_is_a_subtype_of_everything() {
        let mut arena = TypeArena::new();
        let number = arena.number();
        let a = arena.alloc(Type::StringLiteral("a".into()));
        let b = arena.alloc(Type::StringLiteral("b".into()));
        let kind_a = object(&mut arena, &[("kind", a, false)]);
        let kind_b = object(&mut arena, &[("kind", b, false)]);
        let never_like = both(&mut arena, &[kind_a, kind_b]);

        assert!(is_subtype(&arena, never_like, number));
        assert!(is_subtype(&arena, never_like, kind_a));
        assert!(!is_subtype(&arena, number, never_like));
    }

    #[test]
    fn a_function_intersection_is_a_subtype_of_each_function_but_not_the_reverse() {
        let mut arena = TypeArena::new();
        let (number, string) = (arena.number(), arena.string());
        let takes_number = function(&mut arena, number, string);
        let takes_string = function(&mut arena, string, number);
        let overloaded = both(&mut arena, &[takes_number, takes_string]);

        assert!(is_subtype(&arena, overloaded, takes_number));
        assert!(is_subtype(&arena, overloaded, takes_string));
        assert!(!is_subtype(&arena, takes_number, overloaded));
    }

    #[test]
    fn a_branded_primitive_goes_to_its_primitive_and_not_back() {
        let mut arena = TypeArena::new();
        let string = arena.string();
        let id = arena.alloc(Type::StringLiteral("id".into()));
        let brand = object(&mut arena, &[("__brand", id, false)]);
        let user_id = both(&mut arena, &[string, brand]);

        assert!(is_subtype(&arena, user_id, string));
        assert!(!is_subtype(&arena, string, user_id));
    }

    #[test]
    fn the_two_orders_of_an_intersection_are_assignable_both_ways() {
        let mut arena = TypeArena::new();
        let (a, b, ab) = pair(&mut arena);
        let ba = both(&mut arena, &[b, a]);

        assert_ne!(ab, ba);
        assert!(is_subtype(&arena, ab, ba));
        assert!(is_subtype(&arena, ba, ab));
    }

    #[test]
    fn an_intersection_with_a_type_parameter_is_assignable_to_that_parameter() {
        let mut arena = TypeArena::new();
        let number = arena.number();
        let t = arena.alloc(Type::GenericParameter(
            TypeParameterId::new(1, 0),
            "T".into(),
            None,
        ));
        let a = object(&mut arena, &[("a", number, false)]);
        let t_and_a = both(&mut arena, &[t, a]);

        assert!(is_subtype(&arena, t_and_a, t));
        assert!(is_subtype(&arena, t_and_a, a));
        assert!(!is_subtype(&arena, t, t_and_a));
    }
}
