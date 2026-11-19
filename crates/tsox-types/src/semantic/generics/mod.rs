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

mod infer;
mod instantiate;
mod reduce;
pub use self::infer::*;
pub use self::instantiate::*;
pub use self::reduce::*;

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
            order: Default::default(),
            name: name.into(),
            type_id,
            optional: false,
            is_method: false,
            modifiers: Default::default(),
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

    #[test]
    fn substitution_stops_at_the_instantiation_depth_limit() {
        // This is deliberately a deep *generic* shape, not merely a deep concrete type:
        // every array layer contains T and therefore participates in substitution.
        let _ = crate::stack_guard::take_cut();
        let mut arena = TypeArena::new();
        let id = TypeParameterId::new(70, 0);
        let mut ty = arena.alloc(Type::GenericParameter(id, "T".into(), None));
        let number = arena.number();

        for _ in 0..MAX_INSTANTIATION_DEPTH {
            ty = arena.alloc(Type::Array(ty));
        }

        let rewritten = substitute_bound_type_params(&mut arena, ty, &[(id, number)]);
        assert!(
            crate::stack_guard::take_cut(),
            "substitution must record that the depth budget was reached"
        );

        // The outer shape remains useful, while the point beyond the budget is the
        // error type. Walk iteratively so the test itself does not recurse.
        let mut current = rewritten;
        for _ in 0..MAX_INSTANTIATION_DEPTH {
            let Type::Array(element) = arena.get(current) else {
                panic!("expected the array shape to be retained up to the depth limit");
            };
            current = *element;
        }
        assert_eq!(current, arena.error());
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
