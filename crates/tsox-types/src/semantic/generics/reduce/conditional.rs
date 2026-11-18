//! Reduction of conditional types (`T extends U ? X : Y`), distribution and `infer` binding.
use super::*;

/// Reduces a conditional type when its check and extends types are concrete.
/// Otherwise the symbolic node is retained and revisited by generic substitution.
/// This covers distributive conditionals and the structural `infer` patterns most
/// commonly used by utility types without pretending an unresolved T is concrete
/// merely because its constraint happens to fit.
pub fn resolve_conditional_type(arena: &mut TypeArena, conditional: ConditionalType) -> TypeId {
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

pub(super) fn has_unbound_parameters(
    arena: &TypeArena,
    type_id: TypeId,
    bound: &[TypeParameterId],
) -> bool {
    free_generic_params(arena, type_id)
        .iter()
        .any(|(id, _)| !bound.contains(id))
}

pub(super) fn substitute_distributive_parameter(
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

pub(super) fn evaluate_conditional_branch(
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
    } else if let (Some(actual_key), Some(actual_value), Some(expected_key), Some(expected_value)) = (
        arena.record_key_type(check_type),
        arena.record_value_type(check_type),
        arena.record_key_type(extends_type),
        arena.record_value_type(extends_type),
    ) {
        // Record<K, V> is a required mapped shape. A source record with key domain K1
        // supplies every key in a target domain K2 only when K2 is a subset of K1;
        // values flow covariantly. Do not erase K and compare V alone.
        crate::subtyping::is_subtype(arena, expected_key, actual_key)
            && crate::subtyping::is_subtype(arena, actual_value, expected_value)
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

pub(super) fn infer_constraints_hold(
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
