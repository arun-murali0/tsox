// Hashes a type in the arena (LLD 3.2) by walking it and feeding `stable_hash`. This is
// the only place that reads arena ids for identity.
//
// The walk stops at a declaration: a `Ref` or an `App` contributes the DeclKey of its
// declaration and never the declaration's body, so a recursive interface hashes without
// recursion and the walk is as deep as one declaration's own nesting. A `Named` is
// peeled, because an alias is transparent for identity.
//
// None means the type cannot be given a stable identity yet: it mentions a declaration
// or a type parameter the resolver has no key for. The caller treats the export as
// unsupported (incomplete, never wrong).

use crate::arena::{DeclSlot, TypeArena, TypeId};
use crate::keys::{DeclKey, NodeHash, TypeParamKey};
use crate::stable_hash::{NodeIn, ParamIn, Prim, PropIn, TupleIn, hash_node};
use crate::types::{Type, TypeParameterId};

/// Maps run-local ids to stable keys. The unit that owns the declarations implements it;
/// built-in library slots resolve to `DeclKey::builtin(name)`.
pub(crate) trait KeyResolver {
    fn decl_key(&self, slot: DeclSlot) -> Option<DeclKey>;
    /// The name the declaration prints under, carried so an importer can print it.
    fn decl_name(&self, slot: DeclSlot) -> Option<String>;
    fn type_param_key(&self, id: TypeParameterId) -> Option<TypeParamKey>;
    /// The name and constraint of an `infer` binder, which have one canonical answer per
    /// binder even where the checker holds several nodes for it (a placeholder before its
    /// constraint was read, and the constrained one). None for anything that is not one.
    fn infer_binder(&self, _id: TypeParameterId) -> Option<InferBinder> {
        None
    }
}

#[derive(Clone, Debug)]
pub(crate) struct InferBinder {
    pub name: String,
    pub bound: Option<TypeId>,
}

pub(crate) struct StableHasher<'a, R: KeyResolver> {
    arena: &'a TypeArena,
    resolver: &'a R,
    // Per TypeId, filled lazily: only types that reach an export boundary are hashed.
    // Some(None) records a type that cannot be hashed, so it is not retried.
    cache: Vec<Option<Option<NodeHash>>>,
    // Whether parameter names enter the hash. Off for identity (keys, SigHash), on for
    // the export's own de-duplication, which must not merge two functions whose
    // parameters are named differently.
    names: bool,
}

impl<'a, R: KeyResolver> StableHasher<'a, R> {
    pub(crate) fn new(arena: &'a TypeArena, resolver: &'a R) -> Self {
        Self {
            arena,
            resolver,
            cache: Vec::new(),
            names: false,
        }
    }

    pub(crate) fn with_names(arena: &'a TypeArena, resolver: &'a R) -> Self {
        Self {
            names: true,
            ..Self::new(arena, resolver)
        }
    }

    pub(crate) fn hash(&mut self, id: TypeId) -> Option<NodeHash> {
        // The walk is as deep as one declaration's own nesting, which a file can make as
        // deep as it likes (`number[][][]...`). Past the stack budget the type has no
        // identity, which is the answer the caller already treats as an export it cannot
        // represent: incomplete, never wrong. Nothing is cached for it here, so a shallower
        // caller that asks again is not told the type is unhashable.
        if crate::stack_guard::exhausted() {
            return None;
        }
        let at = id.index() as usize;
        // Grown on demand, so this needs nothing from the arena but the id.
        if at >= self.cache.len() {
            self.cache.resize(at + 1, None);
        }
        if let Some(done) = self.cache[at] {
            return done;
        }
        let h = self.compute(id);
        self.cache[at] = Some(h);
        h
    }

    fn all(&mut self, ids: &[TypeId]) -> Option<Vec<NodeHash>> {
        ids.iter().map(|&i| self.hash(i)).collect()
    }

    fn compute(&mut self, id: TypeId) -> Option<NodeHash> {
        // `raw`, not `get`: get looks through Ref, Named and App, and identity needs to
        // see them.
        let ty = self.arena.raw(id).clone();
        let node = match &ty {
            Type::Number => return Some(hash_node(&NodeIn::Prim(Prim::Number))),
            Type::String => return Some(hash_node(&NodeIn::Prim(Prim::String))),
            Type::Boolean => return Some(hash_node(&NodeIn::Prim(Prim::Boolean))),
            Type::Null => return Some(hash_node(&NodeIn::Prim(Prim::Null))),
            Type::Undefined => return Some(hash_node(&NodeIn::Prim(Prim::Undefined))),
            Type::Any => return Some(hash_node(&NodeIn::Prim(Prim::Any))),
            Type::Unknown => return Some(hash_node(&NodeIn::Prim(Prim::Unknown))),
            Type::Never => return Some(hash_node(&NodeIn::Prim(Prim::Never))),
            Type::Void => return Some(hash_node(&NodeIn::Prim(Prim::Void))),
            Type::Error => return Some(hash_node(&NodeIn::Prim(Prim::Error))),
            Type::StringLiteral(s) => return Some(hash_node(&NodeIn::StrLit(s))),
            Type::NumberLiteral(n) => return Some(hash_node(&NodeIn::NumLit(n.to_bits()))),
            Type::BooleanLiteral(b) => return Some(hash_node(&NodeIn::BoolLit(*b))),

            Type::Array(el) => {
                let el = self.hash(*el)?;
                return Some(hash_node(&NodeIn::Array(el)));
            }
            Type::Union(members) => {
                let hs = self.all(members)?;
                return Some(hash_node(&NodeIn::Union(&hs)));
            }
            Type::Intersection(members) => {
                let hs = self.all(members)?;
                return Some(hash_node(&NodeIn::Intersection(&hs)));
            }
            Type::Conditional(conditional) => {
                // The `infer` binders are part of the type: which key each one has and what it
                // is constrained to. A reference to one hashes as that key (`TypeParam`), so
                // two scopes that both say `infer U` are two different binders.
                let mut infers = Vec::with_capacity(conditional.infer_params.len());
                for id in &conditional.infer_params {
                    let key = self.resolver.type_param_key(*id)?;
                    let bound = match self.resolver.infer_binder(*id)?.bound {
                        Some(bound) => Some(self.hash(bound)?),
                        None => None,
                    };
                    infers.push((key, bound));
                }
                let check = self.hash(conditional.check_type)?;
                let extends = self.hash(conditional.extends_type)?;
                let true_type = self.hash(conditional.true_type)?;
                let false_type = self.hash(conditional.false_type)?;
                return Some(hash_node(&NodeIn::Conditional {
                    check,
                    extends,
                    true_type,
                    false_type,
                    distributive: conditional.distributive_param.is_some(),
                    infers: &infers,
                }));
            }
            Type::Tuple(elements) => {
                let mut parts = Vec::with_capacity(elements.len());
                for e in elements {
                    parts.push(TupleIn {
                        optional: e.optional,
                        rest: e.rest,
                        ty: self.hash(e.type_id)?,
                    });
                }
                return Some(hash_node(&NodeIn::Tuple(&parts)));
            }
            Type::Object(object) => {
                let mut hs = Vec::with_capacity(object.properties.len());
                for p in object.properties.iter() {
                    hs.push(self.hash(p.type_id)?);
                }
                let props: Vec<PropIn<'_>> = object
                    .properties
                    .iter()
                    .zip(hs)
                    .map(|(p, ty)| PropIn {
                        name: &p.name,
                        optional: p.optional,
                        is_method: p.is_method,
                        ty,
                    })
                    .collect();
                return Some(hash_node(&NodeIn::Object {
                    flags: 0,
                    props: &props,
                }));
            }
            Type::Function(f) => {
                let mut params = Vec::with_capacity(f.params.len());
                for p in &f.params {
                    params.push(ParamIn {
                        name: if self.names { p.name.as_deref() } else { None },
                        optional: p.optional,
                        rest: p.rest,
                        ty: self.hash(p.type_id)?,
                    });
                }
                let ret = self.hash(f.return_type)?;
                let mut tparams = Vec::with_capacity(f.type_params.len());
                let mut bounds = Vec::with_capacity(f.type_params.len());
                let mut defaults = Vec::with_capacity(f.type_params.len());
                for parameter in &f.type_params {
                    tparams.push(self.resolver.type_param_key(parameter.id)?);
                    bounds.push(match parameter.constraint {
                        Some(id) => Some(self.hash(id)?),
                        None => None,
                    });
                    defaults.push(match parameter.default {
                        Some(id) => Some(self.hash(id)?),
                        None => None,
                    });
                }
                return Some(hash_node(&NodeIn::Function {
                    params: &params,
                    ret,
                    is_untyped: f.is_untyped,
                    tparams: &tparams,
                    bounds: &bounds,
                    defaults: &defaults,
                }));
            }

            Type::GenericParameter(param, _, _) => {
                NodeIn::TypeParam(self.resolver.type_param_key(*param)?)
            }
            Type::Ref(slot) => NodeIn::DeclRef(self.resolver.decl_key(*slot)?),
            // An alias is transparent for identity: hash what it stands for.
            Type::Named(_, inner) => return self.hash(*inner),
            Type::App(slot, args, _body) => {
                let key = self.resolver.decl_key(*slot)?;
                let hs = self.all(args)?;
                return Some(hash_node(&NodeIn::App(key, &hs)));
            }
        };
        Some(hash_node(&node))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{DeclKind, ModuleKey};
    use crate::types::{ObjectType, PropertyEntry};
    use std::rc::Rc;

    // Every slot gets the key of its position-independent name, set by the test.
    struct Keys(Vec<(DeclSlot, DeclKey)>);

    impl KeyResolver for Keys {
        fn decl_key(&self, slot: DeclSlot) -> Option<DeclKey> {
            self.0.iter().find(|(s, _)| *s == slot).map(|(_, k)| *k)
        }
        fn decl_name(&self, _: DeclSlot) -> Option<String> {
            None
        }
        fn type_param_key(&self, _: TypeParameterId) -> Option<TypeParamKey> {
            None
        }
    }

    fn hash_in(arena: &TypeArena, r: &Keys, id: TypeId) -> Option<NodeHash> {
        StableHasher::new(arena, r).hash(id)
    }

    fn prop(name: &str, type_id: TypeId) -> PropertyEntry {
        PropertyEntry {
            name: Rc::from(name),
            type_id,
            optional: false,
            is_method: false,
        }
    }

    #[test]
    fn creation_order_does_not_change_the_hash() {
        let none = Keys(vec![]);

        let mut a = TypeArena::new();
        let (n, s) = (a.number(), a.string());
        let arr_a = a.alloc(Type::Array(n));
        let un_a = a.alloc_union(vec![arr_a, s]);

        // A different creation order, with an unrelated type in between.
        let mut b = TypeArena::new();
        let (n, s) = (b.number(), b.string());
        let _noise = b.alloc(Type::Array(s));
        let arr_b = b.alloc(Type::Array(n));
        let un_b = b.alloc_union(vec![s, arr_b]);

        assert_eq!(hash_in(&a, &none, un_a), hash_in(&b, &none, un_b));
        assert!(hash_in(&a, &none, un_a).is_some());
    }

    #[test]
    fn object_property_order_does_not_change_the_hash() {
        let none = Keys(vec![]);
        let mut a = TypeArena::new();
        let (n, s) = (a.number(), a.string());
        let o1 = a.alloc(Type::Object(ObjectType {
            properties: Rc::from(vec![prop("x", n), prop("y", s)]),
        }));
        let o2 = a.alloc(Type::Object(ObjectType {
            properties: Rc::from(vec![prop("y", s), prop("x", n)]),
        }));
        assert_eq!(hash_in(&a, &none, o1), hash_in(&a, &none, o2));
    }

    #[test]
    fn intersection_order_changes_the_hash() {
        let none = Keys(vec![]);
        let mut a = TypeArena::new();
        let x = a.alloc(Type::Object(ObjectType {
            properties: Rc::from(vec![prop("x", a.number())]),
        }));
        let y = a.alloc(Type::Object(ObjectType {
            properties: Rc::from(vec![prop("y", a.string())]),
        }));
        let xy = a.alloc_intersection(vec![x, y]).unwrap();
        let yx = a.alloc_intersection(vec![y, x]).unwrap();
        assert_ne!(hash_in(&a, &none, xy), hash_in(&a, &none, yx));
    }

    #[test]
    fn a_recursive_interface_hashes_without_recursion() {
        // interface Node { next: Node | null }
        let mut a = TypeArena::new();
        let node = a.alloc_ref();
        let slot = match a.raw(node) {
            Type::Ref(slot) => *slot,
            _ => unreachable!(),
        };
        let null = a.null();
        let next = a.alloc_union(vec![node, null]);
        let body = a.alloc(Type::Object(ObjectType {
            properties: Rc::from(vec![prop("next", next)]),
        }));
        a.resolve_ref(node, body);

        let key = DeclKey::new(ModuleKey::project("a.ts"), "Node", DeclKind::Interface, 0);
        let r = Keys(vec![(slot, key)]);
        assert!(hash_in(&a, &r, node).is_some());
        // the body mentions the declaration by key, so it terminates too
        assert!(hash_in(&a, &r, body).is_some());
    }

    #[test]
    fn an_unkeyed_declaration_is_not_hashable() {
        let mut a = TypeArena::new();
        let node = a.alloc_ref();
        assert_eq!(hash_in(&a, &Keys(vec![]), node), None);
    }

    #[test]
    fn an_alias_hashes_as_what_it_stands_for() {
        let none = Keys(vec![]);
        let mut a = TypeArena::new();
        let n = a.number();
        let arr = a.alloc(Type::Array(n));
        let slot = a.alloc_name("Scores");
        let named = a.alloc_named(slot, arr);
        assert_eq!(hash_in(&a, &none, named), hash_in(&a, &none, arr));
    }
}
