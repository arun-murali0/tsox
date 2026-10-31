//! Stable entry points for semantic type-system operations used by the AST checker.
//!
//! The checker should depend on these operations rather than on the layout of the
//! current implementation in `generics.rs`. This first layer is deliberately a
//! behavior-preserving adapter: it keeps the existing implementation and caches,
//! while giving the upcoming solver migration one narrow seam to replace.

use super::generics::InferenceVariance;
use crate::arena::{TypeArena, TypeId};
use crate::fxhash::FxHashMap;
use crate::semantic::queries::RelationCache;
use crate::types::{Param, TypeParameterId};

/// Relative strength of a candidate source. Explicit type arguments are not candidates
/// at all: their bindings are locked before inference starts. Contextual candidates are
/// higher priority than ordinary argument candidates; low-priority candidates are kept
/// as fallback evidence if no stronger source exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[allow(dead_code)]
pub(crate) enum InferencePriority {
    Low,
    Normal,
    Contextual,
}

#[derive(Clone, Copy, Debug)]
struct InferenceCandidate {
    id: TypeParameterId,
    ty: TypeId,
    priority: InferencePriority,
    preserves_literals: bool,
    variance: InferenceVariance,
    constraint: Option<TypeId>,
    top_level: bool,
    original: TypeId,
}

/// Mutable state for inferring one generic call or constructor application.
///
/// Structural matching only collects candidates. `finish` resolves candidates after all
/// arguments have been visited, giving us a stable point to apply priorities, defaults and
/// constraints without letting an early argument irrevocably decide the answer.
#[derive(Clone)]
pub(crate) struct GenericInferenceSession {
    bindings: Vec<(TypeParameterId, TypeId)>,
    locked: Vec<TypeParameterId>,
    candidates: Vec<InferenceCandidate>,
}

impl GenericInferenceSession {
    pub(crate) fn new(
        bindings: Vec<(TypeParameterId, TypeId)>,
        locked: Vec<TypeParameterId>,
    ) -> Self {
        Self {
            bindings,
            locked,
            candidates: Vec::new(),
        }
    }

    pub(crate) fn infer_argument(
        &mut self,
        arena: &mut TypeArena,
        parameter_type: TypeId,
        argument_type: TypeId,
        cache: &mut RelationCache,
    ) {
        self.infer_argument_with_priority(
            arena,
            parameter_type,
            argument_type,
            InferencePriority::Normal,
            cache,
        );
    }

    pub(crate) fn infer_argument_with_priority(
        &mut self,
        arena: &mut TypeArena,
        parameter_type: TypeId,
        argument_type: TypeId,
        priority: InferencePriority,
        cache: &mut RelationCache,
    ) {
        let mut candidates = Vec::new();
        super::generics::collect_inference_candidates(
            arena,
            parameter_type,
            argument_type,
            &self.locked,
            &mut candidates,
            cache,
        );
        for (id, ty, variance, preserves_literals, constraint, top_level, original) in candidates {
            if self.locked.contains(&id) {
                continue;
            }
            self.candidates.push(InferenceCandidate {
                id,
                ty,
                priority,
                preserves_literals,
                variance,
                constraint,
                top_level,
                original,
            });
        }
    }

    /// Resolve candidates after all arguments have contributed evidence. Candidates at
    /// the highest source priority participate; covariant candidates win over contravariant
    /// candidates, while contra-only evidence is combined with intersections. The ordinary
    /// argument assignability pass remains responsible for reporting incompatible calls.
    pub(crate) fn finish(
        mut self,
        arena: &mut TypeArena,
        cache: &mut RelationCache,
    ) -> Vec<(TypeParameterId, TypeId)> {
        let mut highest_priority = FxHashMap::default();
        for candidate in &self.candidates {
            highest_priority
                .entry(candidate.id)
                .and_modify(|priority: &mut InferencePriority| {
                    *priority = (*priority).max(candidate.priority);
                })
                .or_insert(candidate.priority);
        }
        // tsc widens a literal candidate only when every candidate for its parameter is
        // top-level, among the candidates that survive priority. With one from inside an object, array or function, the top-level
        // ones keep their written literals, so `f("a", { v: 1 })` infers T as "a" and
        // reports the `1`, where widening early would have said `string`. The nested
        // candidates stay as collected: the types of object literal properties and
        // inferred returns are still literal here, and tsc widens those first.
        let nested: Vec<TypeParameterId> = self
            .candidates
            .iter()
            .filter(|candidate| {
                !candidate.top_level
                    && highest_priority.get(&candidate.id) == Some(&candidate.priority)
            })
            .map(|candidate| candidate.id)
            .collect();
        for candidate in &mut self.candidates {
            if candidate.top_level && nested.contains(&candidate.id) {
                candidate.ty = candidate.original;
            }
        }

        // Resolve each parameter as a group. Covariant evidence takes precedence
        // over contravariant evidence, matching TypeScript's inference priority for
        // output positions. If there is no covariant evidence, contra candidates are
        // combined as an intersection (a greatest lower bound), not arbitrarily
        // whichever argument happened to be visited first.
        let mut parameter_ids = Vec::new();
        for candidate in &self.candidates {
            if !parameter_ids.contains(&candidate.id) {
                parameter_ids.push(candidate.id);
            }
        }
        for &id in &parameter_ids {
            if self.locked.contains(&id) {
                continue;
            }
            let Some(&priority) = highest_priority.get(&id) else {
                continue;
            };
            let selected: Vec<InferenceCandidate> = self
                .candidates
                .iter()
                .copied()
                .filter(|candidate| candidate.id == id && candidate.priority == priority)
                .collect();
            let mut has_covariant = selected
                .iter()
                .any(|candidate| candidate.variance == InferenceVariance::Covariant);
            // tsc prefers the covariant inference only when it is assignable to at least one
            // contravariant candidate; otherwise the contravariant one wins. So an annotated
            // callback `(item: string) => ...` fixes `T` as string, and the earlier `1` is the
            // argument reported.
            if has_covariant
                && selected
                    .iter()
                    .any(|candidate| candidate.variance != InferenceVariance::Covariant)
            {
                let mut trial = self.bindings.clone();
                for candidate in selected
                    .iter()
                    .filter(|c| c.variance == InferenceVariance::Covariant)
                {
                    GenericSolver::merge_inference_candidate_with_constraint(
                        arena,
                        candidate.id,
                        candidate.ty,
                        candidate.preserves_literals,
                        candidate.constraint,
                        &mut trial,
                        cache,
                    );
                }
                if let Some(&(_, covariant)) = trial.iter().find(|(bound, _)| *bound == id) {
                    let fits = selected
                        .iter()
                        .filter(|c| c.variance != InferenceVariance::Covariant)
                        .any(|c| {
                            super::generics::is_subtype_via_cache(arena, cache, covariant, c.ty)
                        });
                    if !fits {
                        has_covariant = false;
                    }
                }
            }
            // Recursive constraint resolution can briefly expose an unconstrained
            // GenericParameter node for the same declaration id. Retain declaration
            // metadata from any candidate so the result does not depend on traversal
            // order through a recursive signature.
            let constraint = selected.iter().find_map(|candidate| candidate.constraint);
            let preserves_literals = selected
                .iter()
                .any(|candidate| candidate.preserves_literals);
            let chosen: Vec<InferenceCandidate> = if has_covariant {
                selected
                    .into_iter()
                    .filter(|candidate| candidate.variance == InferenceVariance::Covariant)
                    .collect()
            } else {
                selected
                    .into_iter()
                    .filter(|candidate| candidate.variance != InferenceVariance::Covariant)
                    .collect()
            };
            for candidate in chosen {
                if candidate.variance == InferenceVariance::Covariant {
                    GenericSolver::merge_inference_candidate_with_constraint(
                        arena,
                        candidate.id,
                        candidate.ty,
                        preserves_literals,
                        constraint.or(candidate.constraint),
                        &mut self.bindings,
                        cache,
                    );
                } else {
                    merge_contravariant_candidate(
                        arena,
                        candidate.id,
                        candidate.ty,
                        &mut self.bindings,
                    );
                }
            }
        }

        // Constraints can depend on parameters whose candidates were visited later.
        // For example, T may be constrained by U while U is inferred from a later
        // argument. The first pass cannot use U as a concrete common root yet, so
        // revisit covariant candidates after all direct bindings exist. A bounded
        // fixed-point pass makes chained dependencies independent of argument order
        // without re-merging contravariant candidates (which would duplicate their
        // intersections). Each pass can resolve at least one additional dependency;
        // the number of declared inference parameters is therefore a sufficient bound.
        for _ in 0..parameter_ids.len() {
            let before = self.bindings.clone();
            for &id in &parameter_ids {
                if self.locked.contains(&id) {
                    continue;
                }
                let Some(&priority) = highest_priority.get(&id) else {
                    continue;
                };
                let selected: Vec<InferenceCandidate> = self
                    .candidates
                    .iter()
                    .copied()
                    .filter(|candidate| candidate.id == id && candidate.priority == priority)
                    .collect();
                if !selected
                    .iter()
                    .any(|candidate| candidate.variance == InferenceVariance::Covariant)
                {
                    continue;
                }
                let preserves_literals = selected
                    .iter()
                    .any(|candidate| candidate.preserves_literals);
                let constraint = selected.iter().find_map(|candidate| candidate.constraint);
                for candidate in selected
                    .into_iter()
                    .filter(|candidate| candidate.variance == InferenceVariance::Covariant)
                {
                    GenericSolver::merge_inference_candidate_with_constraint(
                        arena,
                        candidate.id,
                        candidate.ty,
                        preserves_literals,
                        constraint.or(candidate.constraint),
                        &mut self.bindings,
                        cache,
                    );
                }
            }
            if self.bindings == before {
                break;
            }
        }

        self.bindings
    }
}

fn merge_contravariant_candidate(
    arena: &mut TypeArena,
    id: TypeParameterId,
    candidate: TypeId,
    bindings: &mut Vec<(TypeParameterId, TypeId)>,
) {
    match bindings.iter_mut().find(|(bound, _)| *bound == id) {
        None => bindings.push((id, candidate)),
        Some((_, existing)) if *existing == candidate => {}
        Some((_, existing)) => {
            let merged = arena
                .alloc_intersection(vec![*existing, candidate])
                .unwrap_or_else(|_| arena.error());
            *existing = merged;
        }
    }
}

/// Entry point for type-system operations used while checking generic signatures.
///
/// The facade does not own an arena or cache globally. Mutable inference state belongs
/// to one call in `GenericInferenceSession`; relation caching remains owned by the checker.
pub(crate) struct GenericSolver;

impl GenericSolver {
    /// Collect generic type-parameter bindings from one parameter/argument pair.
    pub(crate) fn infer_type_param_bindings(
        arena: &mut TypeArena,
        param_type: TypeId,
        arg_type: TypeId,
        bindings: &mut Vec<(TypeParameterId, TypeId)>,
        locked: &[TypeParameterId],
        cache: &mut RelationCache,
    ) {
        super::generics::infer_type_param_bindings(
            arena, param_type, arg_type, bindings, locked, cache,
        );
    }

    pub(crate) fn merge_inference_candidate_with_constraint(
        arena: &mut TypeArena,
        id: TypeParameterId,
        candidate: TypeId,
        preserves_literals: bool,
        constraint: Option<TypeId>,
        bindings: &mut Vec<(TypeParameterId, TypeId)>,
        cache: &mut RelationCache,
    ) {
        super::generics::merge_inference_candidate_with_constraint(
            arena,
            id,
            candidate,
            preserves_literals,
            constraint,
            bindings,
            cache,
        );
    }

    /// Find type parameters in declaration order, used for positional explicit args.
    pub(crate) fn ordered_generic_param_ids(
        arena: &TypeArena,
        type_id: TypeId,
        out: &mut Vec<TypeParameterId>,
    ) {
        super::generics::ordered_generic_param_ids(arena, type_id, out);
    }

    /// Collect generic constraints reachable from a signature type.
    pub(crate) fn collect_generic_param_constraints(
        arena: &TypeArena,
        type_id: TypeId,
        out: &mut Vec<(TypeParameterId, String, TypeId)>,
    ) {
        super::generics::collect_generic_param_constraints(arena, type_id, out);
    }

    /// Resolve the expected element type for an argument, including rest parameters.
    pub(crate) fn expected_param_type(
        arena: &TypeArena,
        params: &[Param],
        index: usize,
    ) -> Option<TypeId> {
        super::generics::expected_param_type(arena, params, index)
    }

    /// Whether a type still mentions an uninstantiated type parameter.
    pub(crate) fn contains_type_param(arena: &TypeArena, type_id: TypeId) -> bool {
        super::generics::contains_type_param(arena, type_id)
    }

    /// Substitute bound parameters, preserving any parameters without a binding.
    pub(crate) fn substitute_bound_type_params(
        arena: &mut TypeArena,
        type_id: TypeId,
        bindings: &[(TypeParameterId, TypeId)],
    ) -> TypeId {
        super::generics::substitute_bound_type_params(arena, type_id, bindings)
    }

    /// Substitute parameters, resolving any unbound parameter to `unknown`.
    pub(crate) fn substitute_type_params(
        arena: &mut TypeArena,
        type_id: TypeId,
        bindings: &[(TypeParameterId, TypeId)],
    ) -> TypeId {
        super::generics::substitute_type_params(arena, type_id, bindings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TypeParameterId;

    #[test]
    fn inference_session_does_not_override_explicit_type_arguments() {
        let mut arena = TypeArena::new();
        let id = TypeParameterId::new(0, 0);
        let parameter = arena.alloc(crate::types::Type::GenericParameter(
            id,
            "T".to_string(),
            None,
        ));
        let explicit = arena.string();
        let argument = arena.number();
        let mut session = GenericInferenceSession::new(vec![(id, explicit)], vec![id]);
        let mut cache = RelationCache::default();

        session.infer_argument(&mut arena, parameter, argument, &mut cache);

        assert_eq!(session.finish(&mut arena, &mut cache), vec![(id, explicit)]);
    }

    #[test]
    fn higher_priority_candidate_wins_over_lower_priority_candidate() {
        let mut arena = TypeArena::new();
        let id = TypeParameterId::new(10, 0);
        let parameter = arena.alloc(crate::types::Type::GenericParameter(
            id,
            "T".to_string(),
            None,
        ));
        let number = arena.number();
        let string = arena.string();
        let mut session = GenericInferenceSession::new(Vec::new(), Vec::new());
        let mut cache = RelationCache::default();
        session.infer_argument_with_priority(
            &mut arena,
            parameter,
            string,
            InferencePriority::Low,
            &mut cache,
        );
        session.infer_argument_with_priority(
            &mut arena,
            parameter,
            number,
            InferencePriority::Contextual,
            &mut cache,
        );
        assert_eq!(session.finish(&mut arena, &mut cache), vec![(id, number)]);
    }

    #[test]
    fn ordinary_argument_candidate_beats_deferred_object_fallback() {
        let mut arena = TypeArena::new();
        let id = TypeParameterId::new(15, 0);
        let parameter = arena.alloc(crate::types::Type::GenericParameter(
            id,
            "T".to_string(),
            None,
        ));
        let ordinary = arena.string();
        let deferred_object = arena.number();
        let mut session = GenericInferenceSession::new(Vec::new(), Vec::new());
        let mut cache = RelationCache::default();
        session.infer_argument_with_priority(
            &mut arena,
            parameter,
            deferred_object,
            InferencePriority::Low,
            &mut cache,
        );
        session.infer_argument_with_priority(
            &mut arena,
            parameter,
            ordinary,
            InferencePriority::Normal,
            &mut cache,
        );

        assert_eq!(session.finish(&mut arena, &mut cache), vec![(id, ordinary)]);
    }

    #[test]
    fn same_priority_literal_candidates_use_the_existing_union_policy() {
        let mut arena = TypeArena::new();
        let id = TypeParameterId::new(11, 0);
        let number = arena.number();
        let parameter = arena.alloc(crate::types::Type::GenericParameter(
            id,
            "T".to_string(),
            Some(number),
        ));
        let one = arena.alloc(crate::types::Type::NumberLiteral(1.0));
        let two = arena.alloc(crate::types::Type::NumberLiteral(2.0));
        let mut session = GenericInferenceSession::new(Vec::new(), Vec::new());
        let mut cache = RelationCache::default();
        session.infer_argument(&mut arena, parameter, one, &mut cache);
        session.infer_argument(&mut arena, parameter, two, &mut cache);

        let bindings = session.finish(&mut arena, &mut cache);
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].0, id);
        let crate::types::Type::Union(members) = arena.get(bindings[0].1) else {
            panic!("expected the two literal candidates to be combined");
        };
        assert_eq!(members.len(), 2);
    }

    #[test]
    fn contravariant_candidate_wins_when_the_covariant_one_does_not_fit_it() {
        let mut arena = TypeArena::new();
        let id = TypeParameterId::new(12, 0);
        let t = arena.alloc(crate::types::Type::GenericParameter(id, "T".into(), None));
        let void = arena.void();
        let callback = arena.alloc(crate::types::Type::Function(crate::types::FunctionType {
            params: vec![Param::required(t)],
            return_type: void,
            type_params: Vec::new(),
            is_untyped: false,
        }));
        let number = arena.number();
        let actual_callback =
            arena.alloc(crate::types::Type::Function(crate::types::FunctionType {
                params: vec![Param::required(number)],
                return_type: void,
                type_params: Vec::new(),
                is_untyped: false,
            }));
        let string = arena.string();
        let mut session = GenericInferenceSession::new(Vec::new(), Vec::new());
        let mut cache = RelationCache::default();
        session.infer_argument(&mut arena, callback, actual_callback, &mut cache);
        session.infer_argument(&mut arena, t, string, &mut cache);

        // tsc: `f((x: number) => {}, "a")` infers T = number and reports the string.
        assert_eq!(session.finish(&mut arena, &mut cache), vec![(id, number)]);
    }

    #[test]
    fn covariant_candidate_wins_when_it_fits_a_contravariant_one() {
        let mut arena = TypeArena::new();
        let id = TypeParameterId::new(13, 0);
        let t = arena.alloc(crate::types::Type::GenericParameter(id, "T".into(), None));
        let void = arena.void();
        let callback = arena.alloc(crate::types::Type::Function(crate::types::FunctionType {
            params: vec![Param::required(t)],
            return_type: void,
            type_params: Vec::new(),
            is_untyped: false,
        }));
        let (number, string) = (arena.number(), arena.string());
        let wide = arena.alloc_union(vec![number, string]);
        let actual_callback =
            arena.alloc(crate::types::Type::Function(crate::types::FunctionType {
                params: vec![Param::required(wide)],
                return_type: void,
                type_params: Vec::new(),
                is_untyped: false,
            }));
        let mut session = GenericInferenceSession::new(Vec::new(), Vec::new());
        let mut cache = RelationCache::default();
        session.infer_argument(&mut arena, callback, actual_callback, &mut cache);
        session.infer_argument(&mut arena, t, string, &mut cache);

        // tsc: `h((x: string | number) => {}, "a")` is accepted, with T = string.
        assert_eq!(session.finish(&mut arena, &mut cache), vec![(id, string)]);
    }

    #[test]
    fn unrelated_covariant_candidates_keep_the_leftmost_like_tsc() {
        use crate::types::{ObjectType, PropertyEntry, Type};
        use std::rc::Rc;

        let mut arena = TypeArena::new();
        let id = TypeParameterId::new(14, 0);
        let number = arena.number();
        let animal = arena.alloc(Type::Object(ObjectType::new(vec![PropertyEntry {
            name: Rc::from("x"),
            type_id: number,
            optional: false,
            is_method: false,
        }])));
        let giraffe = arena.alloc(Type::Object(ObjectType::new(vec![
            PropertyEntry {
                name: Rc::from("x"),
                type_id: number,
                optional: false,
                is_method: false,
            },
            PropertyEntry {
                name: Rc::from("neck"),
                type_id: number,
                optional: false,
                is_method: false,
            },
        ])));
        let elephant = arena.alloc(Type::Object(ObjectType::new(vec![
            PropertyEntry {
                name: Rc::from("x"),
                type_id: number,
                optional: false,
                is_method: false,
            },
            PropertyEntry {
                name: Rc::from("trunk"),
                type_id: number,
                optional: false,
                is_method: false,
            },
        ])));
        let t = arena.alloc(Type::GenericParameter(id, "T".into(), Some(animal)));
        let mut session = GenericInferenceSession::new(Vec::new(), Vec::new());
        let mut cache = RelationCache::default();
        session.infer_argument(&mut arena, t, giraffe, &mut cache);
        session.infer_argument(&mut arena, t, elephant, &mut cache);

        let bindings = session.finish(&mut arena, &mut cache);
        assert_eq!(bindings, vec![(id, giraffe)]);
    }

    #[test]
    fn dependent_constraint_is_reconciled_after_later_parameter_inference() {
        use crate::types::{ObjectType, PropertyEntry, Type};
        use std::rc::Rc;

        let mut arena = TypeArena::new();
        let t_id = TypeParameterId::new(40, 0);
        let u_id = TypeParameterId::new(40, 1);
        let number = arena.number();
        let base = arena.alloc(Type::Object(ObjectType::new(vec![PropertyEntry {
            name: Rc::from("x"),
            type_id: number,
            optional: false,
            is_method: false,
        }])));
        let giraffe = arena.alloc(Type::Object(ObjectType::new(vec![
            PropertyEntry {
                name: Rc::from("x"),
                type_id: number,
                optional: false,
                is_method: false,
            },
            PropertyEntry {
                name: Rc::from("neck"),
                type_id: number,
                optional: false,
                is_method: false,
            },
        ])));
        let elephant = arena.alloc(Type::Object(ObjectType::new(vec![
            PropertyEntry {
                name: Rc::from("x"),
                type_id: number,
                optional: false,
                is_method: false,
            },
            PropertyEntry {
                name: Rc::from("trunk"),
                type_id: number,
                optional: false,
                is_method: false,
            },
        ])));
        let u = arena.alloc(Type::GenericParameter(u_id, "U".into(), Some(base)));
        let t = arena.alloc(Type::GenericParameter(t_id, "T".into(), Some(u)));
        let mut session = GenericInferenceSession::new(Vec::new(), Vec::new());
        let mut cache = RelationCache::default();

        // T's candidates arrive before U has a binding. The second pass revisits them
        // once U is inferred. They are unrelated, so T keeps the first (tsc's leftmost
        // candidate) and the call reports the second argument, not a widening to U.
        session.infer_argument(&mut arena, t, giraffe, &mut cache);
        session.infer_argument(&mut arena, t, elephant, &mut cache);
        session.infer_argument(&mut arena, u, base, &mut cache);

        let bindings = session.finish(&mut arena, &mut cache);
        assert_eq!(
            bindings
                .iter()
                .find(|(id, _)| *id == t_id)
                .map(|(_, ty)| *ty),
            Some(giraffe)
        );
        assert_eq!(
            bindings
                .iter()
                .find(|(id, _)| *id == u_id)
                .map(|(_, ty)| *ty),
            Some(base)
        );
    }

    #[test]
    fn method_parameter_and_result_are_separate_candidates_and_the_contravariant_one_wins_when_unfit()
     {
        use crate::types::{FunctionType, ObjectType, PropertyEntry, Type};
        use std::rc::Rc;

        let mut arena = TypeArena::new();
        let id = TypeParameterId::new(15, 0);
        let t = arena.alloc(Type::GenericParameter(id, "T".into(), None));
        let number = arena.number();
        let string = arena.string();

        // The declared method both accepts and returns T. The actual method accepts
        // number but returns string. Inference must retain the distinct directions:
        // its parameter contributes contravariant evidence and its result covariant
        // evidence. Treating the whole method property as invariant incorrectly
        // combines number and string as a contra-only intersection (never).
        let expected_method = arena.alloc(Type::Function(FunctionType {
            params: vec![Param::required(t)],
            return_type: t,
            type_params: Vec::new(),
            is_untyped: false,
        }));
        let actual_method = arena.alloc(Type::Function(FunctionType {
            params: vec![Param::required(number)],
            return_type: string,
            type_params: Vec::new(),
            is_untyped: false,
        }));
        let expected = arena.alloc(Type::Object(ObjectType::new(vec![PropertyEntry {
            name: Rc::from("convert"),
            type_id: expected_method,
            optional: false,
            is_method: true,
        }])));
        let actual = arena.alloc(Type::Object(ObjectType::new(vec![PropertyEntry {
            name: Rc::from("convert"),
            type_id: actual_method,
            optional: false,
            is_method: true,
        }])));

        let mut session = GenericInferenceSession::new(Vec::new(), Vec::new());
        let mut cache = RelationCache::default();
        session.infer_argument(&mut arena, expected, actual, &mut cache);

        let bindings = session.finish(&mut arena, &mut cache);
        // The parameter's number does not fit the result's string, so the contravariant
        // candidate wins, as in tsc: `T = number`, and the method's `string` result is what
        // the assignability pass then reports. Never (an intersection of both) is what the
        // unsplit method property used to give.
        assert_eq!(bindings, vec![(id, number)]);
    }

    #[test]
    fn contravariant_candidates_are_combined_as_an_intersection() {
        let mut arena = TypeArena::new();
        let id = TypeParameterId::new(13, 0);
        let t = arena.alloc(crate::types::Type::GenericParameter(id, "T".into(), None));
        let void = arena.void();
        let callback = arena.alloc(crate::types::Type::Function(crate::types::FunctionType {
            params: vec![Param::required(t)],
            return_type: void,
            type_params: Vec::new(),
            is_untyped: false,
        }));
        let number = arena.number();
        let string = arena.string();
        let number_callback =
            arena.alloc(crate::types::Type::Function(crate::types::FunctionType {
                params: vec![Param::required(number)],
                return_type: void,
                type_params: Vec::new(),
                is_untyped: false,
            }));
        let string_callback =
            arena.alloc(crate::types::Type::Function(crate::types::FunctionType {
                params: vec![Param::required(string)],
                return_type: void,
                type_params: Vec::new(),
                is_untyped: false,
            }));
        let mut session = GenericInferenceSession::new(Vec::new(), Vec::new());
        let mut cache = RelationCache::default();
        session.infer_argument(&mut arena, callback, number_callback, &mut cache);
        session.infer_argument(&mut arena, callback, string_callback, &mut cache);

        let bindings = session.finish(&mut arena, &mut cache);
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].0, id);
        assert!(matches!(
            arena.get(bindings[0].1),
            crate::types::Type::Never
        ));
    }
}
