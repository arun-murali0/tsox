// The canonical node encoding and its Merkle hash (LLD 3.2). Pure: it takes children
// that are already hashed, so it knows nothing about the arena and is tested alone.
// The arena walk that feeds it is in `stable_walk.rs`.
//
// Order independence lives here and nowhere else:
//   Union         members sorted ascending by hash
//   Object        properties sorted by name bytes
//   Intersection  member order kept (it is identity, LLD 1.13)
//   Tuple         element order kept (position is the meaning of a tuple)
// Arena creation order, name numbers and thread timing never reach the stream.

use crate::keys::{DeclKey, NodeHash, StreamWriter, TypeParamKey};

#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash)]
#[repr(u8)]
pub enum Prim {
    Number = 0x01,
    String = 0x02,
    Boolean = 0x03,
    Null = 0x04,
    Undefined = 0x05,
    Any = 0x06,
    Unknown = 0x07,
    Never = 0x08,
    Void = 0x09,
    Error = 0x0A,
}

pub struct PropIn<'a> {
    pub name: &'a str,
    pub optional: bool,
    pub is_method: bool,
    pub ty: NodeHash,
}

pub struct TupleIn {
    pub optional: bool,
    pub rest: bool,
    pub ty: NodeHash,
}

pub struct ParamIn<'a> {
    /// Display only: structural equality ignores a parameter's name (`types.rs`), so the
    /// identity hasher passes None. The export hasher passes it, to keep two functions
    /// that differ only in parameter names apart in the portable form.
    pub name: Option<&'a str>,
    pub optional: bool,
    pub rest: bool,
    pub ty: NodeHash,
}

pub enum NodeIn<'a> {
    Prim(Prim),
    StrLit(&'a str),
    /// The bits of the f64, so -0.0 and 0.0 differ and a NaN literal equals itself.
    NumLit(u64),
    BoolLit(bool),
    Array(NodeHash),
    Union(&'a [NodeHash]),
    Object {
        flags: u8,
        props: &'a [PropIn<'a>],
    },
    Function {
        params: &'a [ParamIn<'a>],
        ret: NodeHash,
        /// An untyped function accepts any call; it is part of the type's identity.
        is_untyped: bool,
        tparams: &'a [TypeParamKey],
        bounds: &'a [Option<NodeHash>],
        defaults: &'a [Option<NodeHash>],
    },
    Intersection(&'a [NodeHash]),
    DeclRef(DeclKey),
    App(DeclKey, &'a [NodeHash]),
    TypeParam(TypeParamKey),
}

// Every variant starts with a tag byte of its own, so two shapes cannot write the same
// stream. The tags are part of the encoding like DeclKind's: never renumber one, bump
// ENC_VERSION instead.
pub fn hash_node(node: &NodeIn<'_>) -> NodeHash {
    let mut w = StreamWriter::new();
    match node {
        NodeIn::Prim(p) => {
            w.u8(*p as u8);
        }
        NodeIn::StrLit(s) => {
            w.u8(0x10).str(s);
        }
        NodeIn::NumLit(bits) => {
            w.u8(0x11).u64(*bits);
        }
        NodeIn::BoolLit(b) => {
            w.u8(0x12).u8(*b as u8);
        }
        NodeIn::Array(el) => {
            w.u8(0x20).raw128(el.as_u128());
        }
        NodeIn::Union(members) => {
            let mut sorted: Vec<u128> = members.iter().map(|h| h.as_u128()).collect();
            sorted.sort_unstable();
            w.u8(0x21).u32(sorted.len() as u32);
            for h in sorted {
                w.raw128(h);
            }
        }
        NodeIn::Object { flags, props } => {
            let mut order: Vec<&PropIn<'_>> = props.iter().collect();
            order.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
            w.u8(0x22).u8(*flags).u32(order.len() as u32);
            for p in order {
                w.str(p.name)
                    .u8(p.optional as u8)
                    .u8(p.is_method as u8)
                    .raw128(p.ty.as_u128());
            }
        }
        NodeIn::Function {
            params,
            ret,
            is_untyped,
            tparams,
            bounds,
            defaults,
        } => {
            w.u8(0x23).u8(*is_untyped as u8).u32(params.len() as u32);
            for p in *params {
                match p.name {
                    Some(n) => w.u8(1).str(n),
                    None => w.u8(0),
                };
                w.u8(p.optional as u8)
                    .u8(p.rest as u8)
                    .raw128(p.ty.as_u128());
            }
            w.raw128(ret.as_u128()).u32(tparams.len() as u32);
            for t in *tparams {
                w.raw128(t.owner.as_u128()).u32(t.index);
            }
            w.u32(bounds.len() as u32);
            for bound in *bounds {
                match bound {
                    Some(hash) => w.u8(1).raw128(hash.as_u128()),
                    None => w.u8(0),
                };
            }
            w.u32(defaults.len() as u32);
            for default in *defaults {
                match default {
                    Some(hash) => w.u8(1).raw128(hash.as_u128()),
                    None => w.u8(0),
                };
            }
        }
        NodeIn::Intersection(members) => {
            w.u8(0x24).u32(members.len() as u32);
            for h in *members {
                w.raw128(h.as_u128());
            }
        }
        NodeIn::DeclRef(k) => {
            w.u8(0x30).raw128(k.as_u128());
        }
        NodeIn::App(k, args) => {
            w.u8(0x31).raw128(k.as_u128()).u32(args.len() as u32);
            for h in *args {
                w.raw128(h.as_u128());
            }
        }
        NodeIn::TypeParam(t) => {
            w.u8(0x32).raw128(t.owner.as_u128()).u32(t.index);
        }
    }
    NodeHash::from_u128(w.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{DeclKind, ModuleKey};

    fn num() -> NodeHash {
        hash_node(&NodeIn::Prim(Prim::Number))
    }
    fn st() -> NodeHash {
        hash_node(&NodeIn::Prim(Prim::String))
    }
    fn bo() -> NodeHash {
        hash_node(&NodeIn::Prim(Prim::Boolean))
    }
    fn key(n: &str) -> DeclKey {
        DeclKey::new(ModuleKey::project("a.ts"), n, DeclKind::Interface, 0)
    }

    #[test]
    fn union_member_order_does_not_matter() {
        let a = hash_node(&NodeIn::Union(&[num(), st(), bo()]));
        let b = hash_node(&NodeIn::Union(&[bo(), num(), st()]));
        assert_eq!(a, b);
    }

    #[test]
    fn object_property_order_does_not_matter() {
        let p = |name, ty| PropIn {
            name,
            optional: false,
            is_method: false,
            ty,
        };
        let a = hash_node(&NodeIn::Object {
            flags: 0,
            props: &[p("x", num()), p("y", st())],
        });
        let b = hash_node(&NodeIn::Object {
            flags: 0,
            props: &[p("y", st()), p("x", num())],
        });
        assert_eq!(a, b);
    }

    #[test]
    fn object_optional_and_method_flags_are_identity() {
        let mk = |optional, is_method| {
            hash_node(&NodeIn::Object {
                flags: 0,
                props: &[PropIn {
                    name: "x",
                    optional,
                    is_method,
                    ty: num(),
                }],
            })
        };
        assert_ne!(mk(false, false), mk(true, false));
        assert_ne!(mk(false, false), mk(false, true));
    }

    #[test]
    fn intersection_member_order_is_identity() {
        let a = hash_node(&NodeIn::Intersection(&[num(), st()]));
        let b = hash_node(&NodeIn::Intersection(&[st(), num()]));
        assert_ne!(a, b);
    }

    #[test]
    fn a_union_is_not_an_intersection_of_the_same_members() {
        assert_ne!(
            hash_node(&NodeIn::Union(&[num(), st()])),
            hash_node(&NodeIn::Intersection(&[num(), st()]))
        );
    }

    #[test]
    fn number_literal_uses_bits() {
        let z = hash_node(&NodeIn::NumLit(0.0f64.to_bits()));
        let nz = hash_node(&NodeIn::NumLit((-0.0f64).to_bits()));
        assert_ne!(z, nz);
        let nan = hash_node(&NodeIn::NumLit(f64::NAN.to_bits()));
        assert_eq!(nan, hash_node(&NodeIn::NumLit(f64::NAN.to_bits())));
    }

    #[test]
    fn function_parts_are_identity() {
        let f = |optional, rest, untyped| {
            hash_node(&NodeIn::Function {
                params: &[ParamIn {
                    name: None,
                    optional,
                    rest,
                    ty: num(),
                }],
                ret: st(),
                is_untyped: untyped,
                tparams: &[],
                bounds: &[],
                defaults: &[],
            })
        };
        let base = f(false, false, false);
        assert_ne!(base, f(true, false, false));
        assert_ne!(base, f(false, true, false));
        assert_ne!(base, f(false, false, true));
    }

    #[test]
    fn a_parameter_name_is_in_the_hash_only_when_given() {
        let f = |name| {
            hash_node(&NodeIn::Function {
                params: &[ParamIn {
                    name,
                    optional: false,
                    rest: false,
                    ty: num(),
                }],
                ret: st(),
                is_untyped: false,
                tparams: &[],
                bounds: &[],
                defaults: &[],
            })
        };
        assert_eq!(f(None), f(None));
        assert_ne!(f(Some("x")), f(Some("y")));
        assert_ne!(f(None), f(Some("x")));
    }

    #[test]
    fn decl_refs_hash_by_identity_only() {
        assert_ne!(
            hash_node(&NodeIn::DeclRef(key("A"))),
            hash_node(&NodeIn::DeclRef(key("B")))
        );
        assert_eq!(
            hash_node(&NodeIn::DeclRef(key("A"))),
            hash_node(&NodeIn::DeclRef(key("A")))
        );
    }

    #[test]
    fn app_argument_order_is_identity() {
        let a = hash_node(&NodeIn::App(key("Map"), &[st(), num()]));
        let b = hash_node(&NodeIn::App(key("Map"), &[num(), st()]));
        assert_ne!(a, b);
    }

    #[test]
    fn type_param_key_separates_owner_and_index() {
        let t = |owner, index| hash_node(&NodeIn::TypeParam(TypeParamKey { owner, index }));
        assert_ne!(t(key("A"), 0), t(key("A"), 1));
        assert_ne!(t(key("A"), 0), t(key("B"), 0));
        // two generic function types inside one alias do not collide
        assert_ne!(t(key("A").child(0), 0), t(key("A").child(1), 0));
    }

    #[test]
    fn decl_key_depends_on_every_field() {
        let m = ModuleKey::project("a.ts");
        let base = DeclKey::new(m, "f", DeclKind::Function, 0);
        assert_ne!(
            base,
            DeclKey::new(ModuleKey::project("b.ts"), "f", DeclKind::Function, 0)
        );
        assert_ne!(base, DeclKey::new(m, "g", DeclKind::Function, 0));
        assert_ne!(base, DeclKey::new(m, "f", DeclKind::Var, 0));
        assert_ne!(base, DeclKey::new(m, "f", DeclKind::Function, 1));
    }

    #[test]
    fn module_kinds_do_not_collide() {
        let ks = [
            ModuleKey::project("x"),
            ModuleKey::external("x"),
            ModuleKey::package("x", "1.0.0", ""),
            ModuleKey::builtin(),
        ];
        for i in 0..ks.len() {
            for j in i + 1..ks.len() {
                assert_ne!(ks[i], ks[j]);
            }
        }
    }

    #[test]
    fn project_path_is_lexical_and_skips_node_modules() {
        use std::path::Path;
        let root = Path::new("/w/proj");
        assert_eq!(
            ModuleKey::project_path(root, Path::new("/w/proj/src/./a.ts")).as_deref(),
            Some("src/a.ts")
        );
        assert_eq!(
            ModuleKey::project_path(root, Path::new("/w/proj/node_modules/p/i.ts")),
            None
        );
        assert_eq!(
            ModuleKey::project_path(root, Path::new("/other/a.ts")),
            None
        );
        assert_eq!(ModuleKey::project_path(root, Path::new("/w/proj")), None);
    }

    #[test]
    fn moving_the_project_keeps_the_key() {
        use std::path::Path;
        let a = ModuleKey::project(
            &ModuleKey::project_path(Path::new("/one"), Path::new("/one/s/a.ts")).unwrap(),
        );
        let b = ModuleKey::project(
            &ModuleKey::project_path(Path::new("/two/x"), Path::new("/two/x/s/a.ts")).unwrap(),
        );
        assert_eq!(a, b);
    }

    // The golden test: if this changes, the encoding changed. Bump ENC_VERSION and
    // regenerate on purpose, never to make a test pass.
    #[test]
    fn golden_hashes() {
        let m = ModuleKey::project("src/a.ts");
        let d = DeclKey::new(m, "Point", DeclKind::Interface, 0);
        let obj = hash_node(&NodeIn::Object {
            flags: 0,
            props: &[
                PropIn {
                    name: "x",
                    optional: false,
                    is_method: false,
                    ty: num(),
                },
                PropIn {
                    name: "y",
                    optional: true,
                    is_method: false,
                    ty: num(),
                },
            ],
        });
        let got = [
            (
                "module",
                m.as_u128(),
                0x3697c3e48be71a530544ed739d96180f_u128,
            ),
            ("decl", d.as_u128(), 0x989b8ba5d89b3a99f453e2557c3315d7),
            (
                "number",
                num().as_u128(),
                0x471b3ceeb93f25c3e065b0bfdf57b3d5,
            ),
            ("object", obj.as_u128(), 0x6563f85e333b2dc4979eb9f44bf6fbe1),
            (
                "app",
                hash_node(&NodeIn::App(d, &[st()])).as_u128(),
                0x54404ace929646097cb0488a9bda57f7,
            ),
        ];
        for (n, v, want) in got {
            assert_eq!(
                v, want,
                "{n}: the encoding changed; bump ENC_VERSION on purpose"
            );
        }
    }
}
