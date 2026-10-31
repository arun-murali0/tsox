// Portable declarations (LLD 3.4, 3.5): a declaration's exported shape as owned data with
// no arena in it, and the walk that rebuilds one in another arena.
//
// Export: `export_decl` walks the declaration's body once, in a canonical order, and emits
// a node vector in which every node refers only to lower indices (children first). Equal
// types therefore serialize identically, so derived `Eq` on `PortableType` means
// something, and shared substructure appears once (a DAG, keyed by content hash).
//
// Import: `Importer` makes one pass over that vector, bottom up, with no recursion. A
// `DeclRef` becomes a `Ref` to a slot made for that key, not expanded in place, so a
// cycle ends where it closes. The importer fetches the declarations a body names until
// none is left pending, which is eager over what is reachable (LLD 3.5 sketched lazy
// expansion; `TypeArena::get` takes `&self` and cannot fetch).
//
// Amendments to LLD 3.4, recorded in 3.2.1: an `App` also carries its instantiated body,
// because rebuilding it would need substitution over a declaration that has no AST
// here; and names travel (a declaration's, a parameter's, a type parameter's) because
// messages print them, while the identity hash leaves parameter names out.

use std::rc::Rc;
use std::sync::Arc;

use crate::arena::{DeclSlot, TypeArena, TypeId};
use crate::fxhash::FxHashMap;
use crate::keys::{DeclKey, DeclKind, NodeHash, SigHash, StreamWriter, TypeParamKey};
use crate::stable_hash::Prim;
use crate::stable_walk::{KeyResolver, StableHasher};
use crate::types::{FileId, FunctionType, ObjectType, Param, PropertyEntry, Type, TypeParameterId};

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct PProp {
    pub name: Box<str>,
    pub optional: bool,
    pub is_method: bool,
    pub ty: u32,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct PTupleElement {
    pub optional: bool,
    pub rest: bool,
    pub ty: u32,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct PParam {
    pub name: Option<Box<str>>,
    pub optional: bool,
    pub rest: bool,
    pub ty: u32,
}

// A node names its children by index into the same vector, always a lower index, so an
// importer rebuilds the type in one bottom-up pass with no recursion (see the header). A
// declaration appears by key (`DeclRef`, `App`), never by body; an `App` also carries its
// instantiated body, which is the amendment to LLD 3.4 the header records.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct PFunctionTypeParam {
    pub key: TypeParamKey,
    pub name: Box<str>,
    pub constraint: Option<u32>,
    pub default: Option<u32>,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum PNode {
    Prim(Prim),
    StrLit(Box<str>),
    NumLit(u64),
    BoolLit(bool),
    Array(u32),
    Union(Box<[u32]>),
    Intersection(Box<[u32]>),
    Object {
        props: Box<[PProp]>,
        flags: u8,
    },
    Function {
        params: Box<[PParam]>,
        ret: u32,
        is_untyped: bool,
        type_params: Box<[PFunctionTypeParam]>,
    },
    DeclRef {
        key: DeclKey,
        name: Box<str>,
    },
    App {
        key: DeclKey,
        name: Box<str>,
        args: Box<[u32]>,
        body: u32,
    },
    TypeParam {
        key: DeclKey,
        index: u32,
        name: Box<str>,
        bound: Option<u32>,
    },
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct PortableType {
    pub nodes: Box<[PNode]>,
    pub root: u32,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct PTypeParam {
    pub name: Box<str>,
    /// Index into the declaration's `body.nodes`.
    pub bound: Option<u32>,
    /// The `= Default` of the parameter, as an index into `body.nodes`. It travels with
    /// the declaration because an importer has no AST to read it from: without it a bare
    /// `Box` for `interface Box<T = string>` could not be told from a missing argument,
    /// and the number of required arguments would be unknown.
    pub default: Option<u32>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PortableDecl {
    pub key: DeclKey,
    pub sig: SigHash,
    pub kind: DeclKind,
    pub name: Box<str>,
    pub type_params: Box<[PTypeParam]>,
    pub body: PortableType,
}

// -- export ---------------------------------------------------------------------------

pub(crate) struct TypeParamInput {
    pub id: TypeParameterId,
    pub name: String,
    pub bound: Option<TypeId>,
    pub default: Option<TypeId>,
}

pub(crate) struct DeclInput<'a> {
    pub key: DeclKey,
    pub name: &'a str,
    pub kind: DeclKind,
    pub body: TypeId,
    pub type_params: &'a [TypeParamInput],
}

fn prim_of(ty: &Type) -> Option<Prim> {
    Some(match ty {
        Type::Number => Prim::Number,
        Type::String => Prim::String,
        Type::Boolean => Prim::Boolean,
        Type::Null => Prim::Null,
        Type::Undefined => Prim::Undefined,
        Type::Any => Prim::Any,
        Type::Unknown => Prim::Unknown,
        Type::Never => Prim::Never,
        Type::Void => Prim::Void,
        Type::Error => Prim::Error,
        _ => return None,
    })
}

struct Exporter<'a, R: KeyResolver> {
    arena: &'a TypeArena,
    resolver: &'a R,
    // Orders unions: by identity hash, which parameter names do not touch.
    ident: StableHasher<'a, R>,
    // De-duplicates nodes: names included, so two functions that differ only in
    // parameter names stay two nodes.
    full: StableHasher<'a, R>,
    nodes: Vec<PNode>,
    memo: FxHashMap<u128, u32>,
}

impl<'a, R: KeyResolver> Exporter<'a, R> {
    fn all(&mut self, ids: &[TypeId]) -> Option<Box<[u32]>> {
        let mut out = Vec::with_capacity(ids.len());
        for &id in ids {
            out.push(self.emit(id)?);
        }
        Some(out.into())
    }

    // Recursion depth is the nesting of one declaration's body: Ref, App and generic
    // parameters stop the walk, so it does not follow a declaration into its members.
    fn emit(&mut self, id: TypeId) -> Option<u32> {
        let ty = self.arena.raw(id).clone();
        // An alias is transparent for identity, so it is transparent here too; the
        // importer puts the name back from the declaration's kind.
        if let Type::Named(_, inner) = &ty {
            return self.emit(*inner);
        }
        let key = self.full.hash(id)?.as_u128();
        if let Some(&at) = self.memo.get(&key) {
            return Some(at);
        }
        let node = match &ty {
            Type::StringLiteral(s) => PNode::StrLit(s.as_str().into()),
            Type::NumberLiteral(n) => PNode::NumLit(n.to_bits()),
            Type::BooleanLiteral(b) => PNode::BoolLit(*b),
            Type::Array(el) => PNode::Array(self.emit(*el)?),
            Type::Union(members) => {
                let mut ordered: Vec<(u128, TypeId)> = Vec::with_capacity(members.len());
                for &m in members {
                    ordered.push((self.ident.hash(m)?.as_u128(), m));
                }
                ordered.sort_by_key(|(h, _)| *h);
                let ids: Vec<TypeId> = ordered.into_iter().map(|(_, m)| m).collect();
                PNode::Union(self.all(&ids)?)
            }
            Type::Intersection(members) => PNode::Intersection(self.all(members)?),
            Type::Object(object) => {
                let mut props: Vec<PProp> = Vec::with_capacity(object.properties.len());
                for p in object.properties.iter() {
                    props.push(PProp {
                        name: (&*p.name).into(),
                        optional: p.optional,
                        is_method: p.is_method,
                        ty: self.emit(p.type_id)?,
                    });
                }
                props.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
                PNode::Object {
                    props: props.into(),
                    flags: 0,
                }
            }
            Type::Function(f) => {
                let mut params = Vec::with_capacity(f.params.len());
                for p in &f.params {
                    params.push(PParam {
                        name: p.name.as_deref().map(Into::into),
                        optional: p.optional,
                        rest: p.rest,
                        ty: self.emit(p.type_id)?,
                    });
                }
                let ret = self.emit(f.return_type)?;
                let mut type_params = Vec::with_capacity(f.type_params.len());
                for parameter in &f.type_params {
                    let key = self.resolver.type_param_key(parameter.id)?;
                    let constraint = match parameter.constraint {
                        Some(id) => Some(self.emit(id)?),
                        None => None,
                    };
                    let default = match parameter.default {
                        Some(id) => Some(self.emit(id)?),
                        None => None,
                    };
                    type_params.push(PFunctionTypeParam {
                        key,
                        name: parameter.name.as_str().into(),
                        constraint,
                        default,
                    });
                }
                PNode::Function {
                    params: params.into(),
                    ret,
                    is_untyped: f.is_untyped,
                    type_params: type_params.into(),
                }
            }
            Type::GenericParameter(param, name, bound) => {
                let k = self.resolver.type_param_key(*param)?;
                let bound = match bound {
                    Some(b) => Some(self.emit(b)?),
                    None => None,
                };
                PNode::TypeParam {
                    key: k.owner,
                    index: k.index,
                    name: name.as_str().into(),
                    bound,
                }
            }
            Type::Ref(slot) => PNode::DeclRef {
                key: self.resolver.decl_key(*slot)?,
                name: self.resolver.decl_name(*slot)?.into(),
            },
            Type::App(slot, args, body) => PNode::App {
                key: self.resolver.decl_key(*slot)?,
                name: self.resolver.decl_name(*slot)?.into(),
                args: self.all(args)?,
                body: self.emit(*body)?,
            },
            other => PNode::Prim(prim_of(other)?),
        };
        let at = self.nodes.len() as u32;
        self.nodes.push(node);
        self.memo.insert(key, at);
        Some(at)
    }
}

/// The signature hash (LLD 3.3): the declaration's own shape, with every other
/// declaration appearing by key only. Type parameter bounds and defaults are part of it
/// (a default changes what a bare reference means, so a dependent must see the edit); a
/// name is not, because it is already in the `DeclKey`.
fn sig_hash(
    kind: DeclKind,
    bounds: &[Option<NodeHash>],
    defaults: &[Option<NodeHash>],
    body: NodeHash,
) -> SigHash {
    let mut w = StreamWriter::new();
    w.u8(0x5A).u8(kind as u8).u32(bounds.len() as u32);
    for b in bounds {
        match b {
            Some(h) => w.u8(1).raw128(h.as_u128()),
            None => w.u8(0),
        };
    }
    for d in defaults {
        match d {
            Some(h) => w.u8(1).raw128(h.as_u128()),
            None => w.u8(0),
        };
    }
    w.raw128(body.as_u128());
    SigHash::from_u128(w.finish())
}

/// None when the body mentions something with no stable key yet. The caller reports the
/// export as unsupported, which makes the unit incomplete and never makes it wrong.
pub(crate) fn export_decl<R: KeyResolver>(
    arena: &TypeArena,
    resolver: &R,
    input: &DeclInput<'_>,
) -> Option<PortableDecl> {
    let mut ident = StableHasher::new(arena, resolver);
    let body_hash = ident.hash(input.body)?;
    let mut bound_hashes = Vec::with_capacity(input.type_params.len());
    let mut default_hashes = Vec::with_capacity(input.type_params.len());
    for tp in input.type_params {
        bound_hashes.push(match tp.bound {
            Some(b) => Some(ident.hash(b)?),
            None => None,
        });
        default_hashes.push(match tp.default {
            Some(d) => Some(ident.hash(d)?),
            None => None,
        });
    }

    let mut ex = Exporter {
        arena,
        resolver,
        ident,
        full: StableHasher::with_names(arena, resolver),
        nodes: Vec::new(),
        memo: FxHashMap::default(),
    };
    let root = ex.emit(input.body)?;
    let mut type_params = Vec::with_capacity(input.type_params.len());
    for tp in input.type_params {
        let bound = match tp.bound {
            Some(b) => Some(ex.emit(b)?),
            None => None,
        };
        let default = match tp.default {
            Some(d) => Some(ex.emit(d)?),
            None => None,
        };
        type_params.push(PTypeParam {
            name: tp.name.as_str().into(),
            bound,
            default,
        });
    }
    Some(PortableDecl {
        key: input.key,
        sig: sig_hash(input.kind, &bound_hashes, &default_hashes, body_hash),
        kind: input.kind,
        name: input.name.into(),
        type_params: type_params.into(),
        body: PortableType {
            nodes: ex.nodes.into(),
            root,
        },
    })
}

// -- import ---------------------------------------------------------------------------

/// One type parameter of an imported generic declaration, in this arena's terms. An imported
/// generic has no AST to read its parameter list from, so arity, defaults, bound checks and
/// the `Pair<A, B>` display all read this one record instead of re-deriving it.
#[derive(Clone, Debug)]
pub(crate) struct ImportedParam {
    pub id: TypeParameterId,
    pub name: String,
    /// The GenericParameter node; its bound is the parameter's `extends`.
    pub node: TypeId,
    pub default: Option<TypeId>,
}

pub trait DeclProvider {
    /// The declaration for `key`. A real provider records a dependency edge here
    /// (LLD 6.2) so a unit depends on exactly the declarations it read.
    fn decl(&self, key: DeclKey) -> Option<Arc<PortableDecl>>;
}

/// What a unit may import from: the declarations of the files it depends on, and which
/// of them a module exports under a name. The driver implements it from the dependency
/// units' outputs (LLD 6.2); a test implements it over a few `UnitOutput`s.
pub trait ImportSource: DeclProvider {
    /// The declarations the module that `specifier` names, as written in the importing
    /// file, exports under `name`. More than one when the name is both a type and a
    /// value. Empty when the specifier names no module of the project or the module has
    /// no such export, which leaves the imported name unknown rather than wrong.
    fn export_keys(&self, specifier: &str, name: &str) -> Vec<DeclKey>;

    /// What is known about the module a specifier names, so an import can tell a typo
    /// from an export this checker cannot read yet. The default knows nothing, which
    /// reports nothing.
    fn module_exports(&self, _specifier: &str) -> ModuleExports {
        ModuleExports::Unknown
    }

    /// Every name the module behind `specifier` exports, with the declaration under each,
    /// for `export * from`. `default` is not among them, since a star re-export never
    /// carries it. Empty when the module is unknown; `module_exports` says whether what
    /// is returned is the whole list.
    fn export_all(&self, _specifier: &str) -> Vec<(String, DeclKey)> {
        Vec::new()
    }

    /// Global UMD namespace aliases declared by the project's modules, together with
    /// their public members and (for `export =`) the module's whole exported value.
    /// Implementations return an owned snapshot so an importing unit never borrows project
    /// graph storage while it is mutating its own arena.
    fn global_namespaces(&self) -> Vec<GlobalNamespace> {
        Vec::new()
    }

    /// The project's global declarations of `name`: top-level declarations of script files
    /// and members of `declare global { }` blocks, in a fixed order that does not depend on
    /// which file was read first. Empty when no file declares the name. A name declared
    /// several times (an interface in two files) returns every declaration, for merging.
    fn global_decls(&self, _name: &str) -> Vec<DeclKey> {
        Vec::new()
    }

    /// Whether the project declares `name` globally but its declaration could not be carried
    /// over (see `IncompleteReason::UnrepresentableDeclaration`). The name exists, so a use of
    /// it is not a missing name, but nothing is known about its type.
    /// Whether a script file should build portable declarations for its global names. A
    /// project that has not yet seen a use of any of them says no, and the script lists only
    /// its names; building a declaration for every top-level item of every script is the
    /// cost, and most scripts' names are never used by another file.
    fn collect_global_decls(&self) -> bool {
        true
    }

    fn global_name_unmodelled(&self, _name: &str) -> bool {
        false
    }
}

/// The UMD global binding contributed by one or more project exports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlobalNamespace {
    pub name: String,
    pub members: Vec<(String, DeclKey)>,
    /// The value assigned by `export =`, if present. This can be callable, not merely an
    /// object namespace (for example a CommonJS function exposed as a UMD global).
    pub export_assignment: Option<DeclKey>,
}

/// What an importing unit knows about the module behind a specifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModuleExports {
    /// The project's resolution found no file for the specifier.
    Unresolved,
    /// A file exists, but its exports were not read (not checked, failed, or no key).
    Unknown,
    /// A file was checked. `complete` says whether the export list is the whole list;
    /// a name missing from an incomplete list may be one that was left out.
    Known { complete: bool },
}

/// Rebuilds portable declarations in one arena. One per unit run: it remembers which slot
/// stands for which key, so two uses of an imported declaration share one id, and it is
/// the `KeyResolver` for anything imported, so a re-export can be exported again.
#[derive(Default)]
pub(crate) struct Importer {
    refs: FxHashMap<DeclKey, TypeId>,
    slot_keys: FxHashMap<DeclSlot, (DeclKey, String)>,
    name_slots: FxHashMap<DeclKey, DeclSlot>,
    owners: FxHashMap<DeclKey, u32>,
    tparams: FxHashMap<TypeParameterId, TypeParamKey>,
    tparam_ids: FxHashMap<TypeParamKey, TypeParameterId>,
    pending: Vec<(TypeId, Arc<PortableDecl>)>,
}

impl KeyResolver for Importer {
    fn decl_key(&self, slot: DeclSlot) -> Option<DeclKey> {
        self.slot_keys.get(&slot).map(|(k, _)| *k)
    }
    fn decl_name(&self, slot: DeclSlot) -> Option<String> {
        self.slot_keys.get(&slot).map(|(_, n)| n.clone())
    }
    fn type_param_key(&self, id: TypeParameterId) -> Option<TypeParamKey> {
        self.tparams.get(&id).copied()
    }
}

impl Importer {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// The local type of the declaration under `key`: a `Ref` for an interface or a
    /// class, the alias's body under its name for an alias, the body itself for a
    /// function or a variable. None for a kind that is not portable yet (a
    /// namespace; an enum has two types and comes through `import_enum`) or a key the provider does not have.
    pub(crate) fn import_decl(
        &mut self,
        arena: &mut TypeArena,
        provider: &dyn DeclProvider,
        key: DeclKey,
    ) -> Option<TypeId> {
        let decl = provider.decl(key)?;
        let ty = match decl.kind {
            DeclKind::Interface | DeclKind::Class => self.ref_for(arena, provider, key)?,
            DeclKind::Alias => {
                let body = self.body(arena, provider, &decl);
                if decl.type_params.is_empty() {
                    let slot = self.name_slot(arena, key, &decl.name);
                    arena.alloc_named(slot, body)
                } else {
                    // A generic alias is left unnamed, as the checker leaves it
                    // (namespace::resolve): its body still holds bare type
                    // parameters, and each instantiation names itself. The body is
                    // registered against the declaration's slot, so an application of
                    // it can be traced back to this key when it is exported again.
                    let slot = self.name_slot(arena, key, &decl.name);
                    arena.set_body_origin(body, slot);
                    body
                }
            }
            DeclKind::Function | DeclKind::Var => self.body(arena, provider, &decl),
            DeclKind::Enum | DeclKind::Namespace => return None,
        };
        self.drain(arena, provider);
        Some(ty)
    }

    /// The type position and the value position of the enum under `key`, rebuilt from the
    /// member literals the declaration carries. None for any other kind, or a key the
    /// provider does not have.
    pub(crate) fn import_enum(
        &mut self,
        arena: &mut TypeArena,
        provider: &dyn DeclProvider,
        key: DeclKey,
    ) -> Option<(TypeId, TypeId)> {
        let decl = provider.decl(key)?;
        if decl.kind != DeclKind::Enum {
            return None;
        }
        let body = self.body(arena, provider, &decl);
        let Type::Object(object) = arena.get(body).clone() else {
            return None;
        };
        let members: Vec<(String, TypeId)> = object
            .properties
            .iter()
            .map(|p| (p.name.to_string(), p.type_id))
            .collect();
        self.drain(arena, provider);
        Some(arena.build_enum(&decl.name, members))
    }

    /// The type parameters of the generic declaration under `key`, as local types: what a
    /// namespace needs to instantiate it without an AST. The node is the very
    /// GenericParameter the imported body holds (allocation is interned, so asking again
    /// gives the same id), so substituting it into the base shape finds every use. None
    /// when the provider does not have the declaration.
    pub(crate) fn generic_params(
        &mut self,
        arena: &mut TypeArena,
        provider: &dyn DeclProvider,
        key: DeclKey,
    ) -> Option<Vec<ImportedParam>> {
        let decl = provider.decl(key)?;
        if decl.type_params.is_empty() {
            return Some(Vec::new());
        }
        let ids = self.body_ids(arena, provider, &decl);
        let mut params = Vec::with_capacity(decl.type_params.len());
        for (index, tp) in decl.type_params.iter().enumerate() {
            let id = self.tparam_id(key, index as u32);
            let bound = tp.bound.map(|b| ids[b as usize]);
            let node = arena.alloc(Type::GenericParameter(id, tp.name.to_string(), bound));
            params.push(ImportedParam {
                id,
                name: tp.name.to_string(),
                node,
                default: tp.default.map(|d| ids[d as usize]),
            });
        }
        self.drain(arena, provider);
        Some(params)
    }

    // Only an interface or a class gets a Ref slot here. The declaration is queued on
    // `pending` and its body is read later, so a declaration that names itself finds the
    // Ref already made and the cycle ends where it closes.
    fn ref_for(
        &mut self,
        arena: &mut TypeArena,
        provider: &dyn DeclProvider,
        key: DeclKey,
    ) -> Option<TypeId> {
        if let Some(&r) = self.refs.get(&key) {
            return Some(r);
        }
        let decl = provider.decl(key)?;
        if !matches!(decl.kind, DeclKind::Interface | DeclKind::Class) {
            return None;
        }
        let r = arena.alloc_ref();
        let slot = arena.slot_of(r)?;
        self.refs.insert(key, r);
        self.slot_keys.insert(slot, (key, decl.name.to_string()));
        self.pending.push((r, decl));
        Some(r)
    }

    fn name_slot(&mut self, arena: &mut TypeArena, key: DeclKey, name: &str) -> DeclSlot {
        if let Some(&s) = self.name_slots.get(&key) {
            return s;
        }
        let s = arena.alloc_name(name);
        self.name_slots.insert(key, s);
        self.slot_keys.insert(s, (key, name.to_string()));
        s
    }

    fn tparam_id(&mut self, key: DeclKey, index: u32) -> TypeParameterId {
        let tk = TypeParamKey { owner: key, index };
        if let Some(&id) = self.tparam_ids.get(&tk) {
            return id;
        }
        let next = self.owners.len() as u32;
        let owner = *self.owners.entry(key).or_insert(next);
        // A run-local id: no file has this FileId, and the owner number is this
        // importer's own, so it cannot meet a parameter the checker declared.
        let id = TypeParameterId::with_file(FileId::new(u32::MAX), owner, index);
        self.tparam_ids.insert(tk, id);
        self.tparams.insert(id, tk);
        id
    }

    // Takes declarations off the pending list until none is left, so a cycle of
    // references ends: a key already in `refs` is never queued twice.
    fn drain(&mut self, arena: &mut TypeArena, provider: &dyn DeclProvider) {
        while let Some((reference, decl)) = self.pending.pop() {
            let body = self.body(arena, provider, &decl);
            arena.resolve_ref(reference, body);
            if decl.type_params.is_empty() {
                arena.name_ref(reference, &*decl.name);
            }
        }
    }

    fn body(
        &mut self,
        arena: &mut TypeArena,
        provider: &dyn DeclProvider,
        decl: &PortableDecl,
    ) -> TypeId {
        let ids = self.body_ids(arena, provider, decl);
        ids[decl.body.root as usize]
    }

    // One forward pass: every node refers only to lower indices, so no recursion and no
    // depth limit. Returns the local id of every node, not just the root, because a type
    // parameter's bound and default are nodes of the same vector.
    fn body_ids(
        &mut self,
        arena: &mut TypeArena,
        provider: &dyn DeclProvider,
        decl: &PortableDecl,
    ) -> Vec<TypeId> {
        let mut ids: Vec<TypeId> = Vec::with_capacity(decl.body.nodes.len());
        for node in decl.body.nodes.iter() {
            let id = match node {
                PNode::Prim(p) => match p {
                    Prim::Number => arena.number(),
                    Prim::String => arena.string(),
                    Prim::Boolean => arena.boolean(),
                    Prim::Null => arena.null(),
                    Prim::Undefined => arena.undefined(),
                    Prim::Any => arena.any(),
                    Prim::Unknown => arena.unknown(),
                    Prim::Never => arena.never(),
                    Prim::Void => arena.void(),
                    Prim::Error => arena.error(),
                },
                PNode::StrLit(s) => arena.alloc(Type::StringLiteral(s.to_string())),
                PNode::NumLit(bits) => arena.alloc(Type::NumberLiteral(f64::from_bits(*bits))),
                PNode::BoolLit(b) => arena.alloc(Type::BooleanLiteral(*b)),
                PNode::Array(e) => arena.alloc(Type::Array(ids[*e as usize])),
                PNode::Union(ms) => {
                    arena.alloc_union(ms.iter().map(|&m| ids[m as usize]).collect())
                }
                PNode::Intersection(ms) => {
                    let members = ms.iter().map(|&m| ids[m as usize]).collect();
                    // Too complex to build: Error does not cascade into a mismatch.
                    arena
                        .alloc_intersection(members)
                        .unwrap_or_else(|_| arena.error())
                }
                PNode::Object { props, .. } => {
                    let props = props
                        .iter()
                        .map(|p| PropertyEntry {
                            name: Rc::from(&*p.name),
                            type_id: ids[p.ty as usize],
                            optional: p.optional,
                            is_method: p.is_method,
                        })
                        .collect();
                    arena.alloc(Type::Object(ObjectType::new(props)))
                }
                PNode::Function {
                    params,
                    ret,
                    is_untyped,
                    type_params,
                } => {
                    let params = params
                        .iter()
                        .map(|p| Param {
                            type_id: ids[p.ty as usize],
                            optional: p.optional,
                            rest: p.rest,
                            name: p.name.as_deref().map(Rc::from),
                        })
                        .collect();
                    let mut local_type_params = Vec::with_capacity(type_params.len());
                    for parameter in type_params.iter() {
                        let id = self.tparam_id(parameter.key.owner, parameter.key.index);
                        let constraint = parameter.constraint.map(|at| ids[at as usize]);
                        let default = parameter.default.map(|at| ids[at as usize]);
                        // `<T>() => void` has no child TypeParam node to teach the
                        // importer about T, so the signature metadata registers it here.
                        self.tparams.insert(id, parameter.key);
                        arena.alloc(Type::GenericParameter(
                            id,
                            parameter.name.to_string(),
                            constraint,
                        ));
                        local_type_params.push(crate::types::FunctionTypeParam {
                            id,
                            name: parameter.name.to_string(),
                            constraint,
                            default,
                        });
                    }
                    arena.alloc(Type::Function(FunctionType {
                        params,
                        return_type: ids[*ret as usize],
                        type_params: local_type_params,
                        is_untyped: *is_untyped,
                    }))
                }
                PNode::DeclRef { key, .. } => {
                    // A declaration the provider does not have reads as Error, which
                    // does not cascade: a missed error, never a false one.
                    self.ref_for(arena, provider, *key)
                        .unwrap_or_else(|| arena.error())
                }
                PNode::App {
                    key,
                    name,
                    args,
                    body,
                } => {
                    let slot = if *key == DeclKey::builtin("Promise") {
                        arena.builtin_slot("Promise")
                    } else if *key == DeclKey::builtin("Record") {
                        arena.builtin_slot("Record")
                    } else {
                        // The checker gives every application its own name slot
                        // (type_annotation.rs), so this does too.
                        arena.alloc_name(&**name)
                    };
                    self.slot_keys.insert(slot, (*key, name.to_string()));
                    let args = args.iter().map(|&a| ids[a as usize]).collect();
                    arena.alloc_app(slot, args, ids[*body as usize])
                }
                PNode::TypeParam {
                    key,
                    index,
                    name,
                    bound,
                } => {
                    let id = self.tparam_id(*key, *index);
                    let bound = bound.map(|b| ids[b as usize]);
                    arena.alloc(Type::GenericParameter(id, name.to_string(), bound))
                }
            };
            ids.push(id);
        }
        ids
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::ModuleKey;
    use std::collections::HashMap;

    // The unit's side of the contract in miniature: which slot is which declaration, and
    // which type parameter belongs to which.
    #[derive(Default)]
    struct Keys {
        slots: Vec<(DeclSlot, DeclKey, String)>,
        tparams: Vec<(TypeParameterId, TypeParamKey)>,
    }

    impl KeyResolver for Keys {
        fn decl_key(&self, slot: DeclSlot) -> Option<DeclKey> {
            self.slots
                .iter()
                .find(|(s, ..)| *s == slot)
                .map(|(_, k, _)| *k)
        }
        fn decl_name(&self, slot: DeclSlot) -> Option<String> {
            self.slots
                .iter()
                .find(|(s, ..)| *s == slot)
                .map(|(.., n)| n.clone())
        }
        fn type_param_key(&self, id: TypeParameterId) -> Option<TypeParamKey> {
            self.tparams.iter().find(|(i, _)| *i == id).map(|(_, k)| *k)
        }
    }

    #[derive(Default)]
    struct Provider(HashMap<DeclKey, Arc<PortableDecl>>);

    impl DeclProvider for Provider {
        fn decl(&self, key: DeclKey) -> Option<Arc<PortableDecl>> {
            self.0.get(&key).cloned()
        }
    }

    fn key(name: &str, kind: DeclKind) -> DeclKey {
        DeclKey::new(ModuleKey::project("a.ts"), name, kind, 0)
    }

    fn prop(name: &str, type_id: TypeId) -> PropertyEntry {
        PropertyEntry {
            name: Rc::from(name),
            type_id,
            optional: false,
            is_method: false,
        }
    }

    fn param(name: &str, type_id: TypeId) -> Param {
        Param {
            type_id,
            optional: false,
            rest: false,
            name: Some(Rc::from(name)),
        }
    }

    fn func(arena: &mut TypeArena, params: Vec<Param>, ret: TypeId) -> TypeId {
        arena.alloc(Type::Function(FunctionType {
            params,
            return_type: ret,
            type_params: Vec::new(),
            is_untyped: false,
        }))
    }

    struct Built {
        reference: TypeId,
        body: TypeId,
        key: DeclKey,
    }

    // interface Node { value: number; next: Node | null }
    fn node_interface(arena: &mut TypeArena, keys: &mut Keys) -> Built {
        let k = key("Node", DeclKind::Interface);
        let r = arena.alloc_ref();
        let slot = arena.slot_of(r).unwrap();
        keys.slots.push((slot, k, "Node".into()));
        let null = arena.null();
        let next = arena.alloc_union(vec![r, null]);
        let value = arena.number();
        let body = arena.alloc(Type::Object(ObjectType::new(vec![
            prop("value", value),
            prop("next", next),
        ])));
        arena.resolve_ref(r, body);
        arena.name_ref(r, "Node");
        Built {
            reference: r,
            body,
            key: k,
        }
    }

    fn export(
        arena: &TypeArena,
        keys: &Keys,
        k: DeclKey,
        name: &str,
        kind: DeclKind,
        body: TypeId,
    ) -> PortableDecl {
        let input = DeclInput {
            key: k,
            name,
            kind,
            body,
            type_params: &[],
        };
        export_decl(arena, keys, &input).expect("exportable")
    }

    fn provider_of(decls: &[&PortableDecl]) -> Provider {
        Provider(
            decls
                .iter()
                .map(|d| (d.key, Arc::new((*d).clone())))
                .collect(),
        )
    }

    // function f(x: Node, n?: number): string, and the Node it names
    fn f_and_node(a: &mut TypeArena) -> (PortableDecl, PortableDecl) {
        let mut keys = Keys::default();
        let node = node_interface(a, &mut keys);
        let (num, st) = (a.number(), a.string());
        let f_body = func(
            a,
            vec![
                param("x", node.reference),
                Param {
                    optional: true,
                    ..param("n", num)
                },
            ],
            st,
        );
        let node_decl = export(a, &keys, node.key, "Node", DeclKind::Interface, node.body);
        let f_decl = export(
            a,
            &keys,
            key("f", DeclKind::Function),
            "f",
            DeclKind::Function,
            f_body,
        );
        (f_decl, node_decl)
    }

    #[test]
    fn export_import_export_round_trips() {
        let mut a = TypeArena::new();
        let (f, node) = f_and_node(&mut a);
        let provider = provider_of(&[&f, &node]);

        let mut b = TypeArena::new();
        let mut importer = Importer::new();
        let imported = importer
            .import_decl(&mut b, &provider, f.key)
            .expect("imports");

        let again = export_decl(
            &b,
            &importer,
            &DeclInput {
                key: f.key,
                name: "f",
                kind: DeclKind::Function,
                body: imported,
                type_params: &[],
            },
        )
        .expect("exports again");
        assert_eq!(again, f);
        assert_eq!(again.sig, f.sig);
    }

    #[test]
    fn two_uses_of_one_imported_declaration_share_one_id() {
        // function g(a: Node, b: Node): void
        let mut a = TypeArena::new();
        let mut keys = Keys::default();
        let node = node_interface(&mut a, &mut keys);
        let void = a.void();
        let g_body = func(
            &mut a,
            vec![param("a", node.reference), param("b", node.reference)],
            void,
        );
        let node_decl = export(&a, &keys, node.key, "Node", DeclKind::Interface, node.body);
        let g = export(
            &a,
            &keys,
            key("g", DeclKind::Function),
            "g",
            DeclKind::Function,
            g_body,
        );
        let provider = provider_of(&[&g, &node_decl]);

        let mut b = TypeArena::new();
        let mut importer = Importer::new();
        let ty = importer.import_decl(&mut b, &provider, g.key).unwrap();
        let Type::Function(f) = b.get(ty).clone() else {
            panic!("a function")
        };
        assert_eq!(f.params[0].type_id, f.params[1].type_id);
        assert_eq!(b.display_name(f.params[0].type_id), Some("Node"));
    }

    #[test]
    fn a_recursive_interface_is_imported_with_its_cycle_intact() {
        let mut a = TypeArena::new();
        let mut keys = Keys::default();
        let node = node_interface(&mut a, &mut keys);
        let decl = export(&a, &keys, node.key, "Node", DeclKind::Interface, node.body);
        let provider = provider_of(&[&decl]);

        let mut b = TypeArena::new();
        let mut importer = Importer::new();
        let r = importer.import_decl(&mut b, &provider, node.key).unwrap();
        let Type::Object(object) = b.get(r).clone() else {
            panic!("an object")
        };
        assert_eq!(object.properties.len(), 2);
        // `next` is `Node | null`, and that Node is this same declaration
        let next = object
            .properties
            .iter()
            .find(|p| &*p.name == "next")
            .unwrap();
        let Type::Union(members) = b.get(next.type_id).clone() else {
            panic!("a union")
        };
        assert!(members.contains(&r));
    }

    #[test]
    fn export_does_not_depend_on_creation_order() {
        let build = |noise_first: bool| {
            let mut a = TypeArena::new();
            let mut keys = Keys::default();
            if noise_first {
                let s = a.string();
                let _ = a.alloc(Type::Array(s));
                let _ = a.alloc_union(vec![s, a.null()]);
            }
            let node = node_interface(&mut a, &mut keys);
            let (num, st) = (a.number(), a.string());
            let u = a.alloc_union(vec![st, num]);
            let body = func(&mut a, vec![param("x", node.reference), param("y", u)], st);
            export(
                &a,
                &keys,
                key("h", DeclKind::Function),
                "h",
                DeclKind::Function,
                body,
            )
        };
        assert_eq!(build(false), build(true));
    }

    #[test]
    fn union_member_order_does_not_change_the_export() {
        let one = |first_string: bool| {
            let mut a = TypeArena::new();
            let (num, st) = (a.number(), a.string());
            let u = if first_string {
                a.alloc_union(vec![st, num])
            } else {
                a.alloc_union(vec![num, st])
            };
            export(
                &a,
                &Keys::default(),
                key("v", DeclKind::Var),
                "v",
                DeclKind::Var,
                u,
            )
        };
        assert_eq!(one(true), one(false));
    }

    #[test]
    fn a_signature_edit_changes_the_sig_hash_and_nothing_else_does() {
        let sig = |ret_string: bool, param_name: &str| {
            let mut a = TypeArena::new();
            let (num, st) = (a.number(), a.string());
            let ret = if ret_string { st } else { num };
            let body = func(&mut a, vec![param(param_name, num)], ret);
            export(
                &a,
                &Keys::default(),
                key("f", DeclKind::Function),
                "f",
                DeclKind::Function,
                body,
            )
            .sig
        };
        assert_eq!(sig(true, "x"), sig(true, "x"));
        assert_ne!(sig(true, "x"), sig(false, "x"));
        // a parameter's name is display only, as it is for type equality
        assert_eq!(sig(true, "x"), sig(true, "y"));
    }

    #[test]
    fn parameter_names_travel_though_they_are_not_identity() {
        let one = |name: &str| {
            let mut a = TypeArena::new();
            let (num, st) = (a.number(), a.string());
            let body = func(&mut a, vec![param(name, num)], st);
            export(
                &a,
                &Keys::default(),
                key("f", DeclKind::Function),
                "f",
                DeclKind::Function,
                body,
            )
        };
        let (x, y) = (one("x"), one("y"));
        assert_eq!(x.sig, y.sig);
        assert_ne!(x, y);
        let PNode::Function { params, .. } = &x.body.nodes[x.body.root as usize] else {
            panic!()
        };
        assert_eq!(params[0].name.as_deref(), Some("x"));
    }

    #[test]
    fn a_type_parameter_bound_is_part_of_the_signature() {
        // function id<T extends string>(x: T): T   vs   <T extends number>
        let sig = |bound_string: bool| {
            let mut a = TypeArena::new();
            let bound = if bound_string { a.string() } else { a.number() };
            let owner = key("id", DeclKind::Function);
            let tp = TypeParameterId::with_file(FileId::ROOT, 7, 0);
            let t = a.alloc(Type::GenericParameter(tp, "T".into(), Some(bound)));
            let body = func(&mut a, vec![param("x", t)], t);
            let keys = Keys {
                slots: vec![],
                tparams: vec![(tp, TypeParamKey { owner, index: 0 })],
            };
            let tps = [TypeParamInput {
                id: tp,
                name: "T".into(),
                bound: Some(bound),
                default: None,
            }];
            let d = export_decl(
                &a,
                &keys,
                &DeclInput {
                    key: owner,
                    name: "id",
                    kind: DeclKind::Function,
                    body,
                    type_params: &tps,
                },
            )
            .unwrap();
            (d.sig, d)
        };
        let (s1, d1) = sig(true);
        let (s2, _) = sig(false);
        assert_ne!(s1, s2);
        assert_eq!(&*d1.type_params[0].name, "T");
        assert!(d1.type_params[0].bound.is_some());
    }

    #[test]
    fn a_generic_application_keeps_its_arguments_and_its_name() {
        // interface Box<T> { value: T };  function use(b: Box<number>): void
        let mut a = TypeArena::new();
        let box_key = key("Box", DeclKind::Interface);
        let tp = TypeParameterId::with_file(FileId::ROOT, 3, 0);
        let t = a.alloc(Type::GenericParameter(tp, "T".into(), None));
        let num = a.number();
        let generic_body = a.alloc(Type::Object(ObjectType::new(vec![prop("value", t)])));
        let applied_body = a.alloc(Type::Object(ObjectType::new(vec![prop("value", num)])));
        let slot = a.alloc_name("Box");
        let app = a.alloc_app(slot, vec![num], applied_body);
        let void = a.void();
        let use_body = func(&mut a, vec![param("b", app)], void);

        let keys = Keys {
            slots: vec![(slot, box_key, "Box".into())],
            tparams: vec![(
                tp,
                TypeParamKey {
                    owner: box_key,
                    index: 0,
                },
            )],
        };
        let tps = [TypeParamInput {
            id: tp,
            name: "T".into(),
            bound: None,
            default: None,
        }];
        let box_decl = export_decl(
            &a,
            &keys,
            &DeclInput {
                key: box_key,
                name: "Box",
                kind: DeclKind::Interface,
                body: generic_body,
                type_params: &tps,
            },
        )
        .unwrap();
        let use_decl = export(
            &a,
            &keys,
            key("use", DeclKind::Function),
            "use",
            DeclKind::Function,
            use_body,
        );
        let provider = provider_of(&[&box_decl, &use_decl]);

        let mut b = TypeArena::new();
        let mut importer = Importer::new();
        let ty = importer
            .import_decl(&mut b, &provider, use_decl.key)
            .unwrap();
        let Type::Function(f) = b.get(ty).clone() else {
            panic!()
        };
        let (name, args) = b
            .app_parts(f.params[0].type_id)
            .expect("still an application");
        assert_eq!(name, "Box");
        assert_eq!(args.len(), 1);
        // and it exports again to the same thing
        let again = export_decl(
            &b,
            &importer,
            &DeclInput {
                key: use_decl.key,
                name: "use",
                kind: DeclKind::Function,
                body: ty,
                type_params: &[],
            },
        )
        .unwrap();
        assert_eq!(again, use_decl);
    }

    #[test]
    fn an_alias_is_imported_under_its_name() {
        // type Scores = number[]
        let mut a = TypeArena::new();
        let num = a.number();
        let arr = a.alloc(Type::Array(num));
        let slot = a.alloc_name("Scores");
        let named = a.alloc_named(slot, arr);
        let k = key("Scores", DeclKind::Alias);
        let keys = Keys {
            slots: vec![(slot, k, "Scores".into())],
            tparams: vec![],
        };
        let decl = export(&a, &keys, k, "Scores", DeclKind::Alias, named);
        // transparent in the portable form
        assert!(matches!(
            decl.body.nodes[decl.body.root as usize],
            PNode::Array(_)
        ));

        let provider = provider_of(&[&decl]);
        let mut b = TypeArena::new();
        let mut importer = Importer::new();
        let ty = importer.import_decl(&mut b, &provider, k).unwrap();
        assert_eq!(b.display_name(ty), Some("Scores"));
        assert!(matches!(b.get(ty), Type::Array(_)));
    }

    #[test]
    fn something_without_a_key_is_not_exported() {
        let mut a = TypeArena::new();
        let r = a.alloc_ref();
        let void = a.void();
        let body = func(&mut a, vec![param("x", r)], void);
        let input = DeclInput {
            key: key("f", DeclKind::Function),
            name: "f",
            kind: DeclKind::Function,
            body,
            type_params: &[],
        };
        assert!(export_decl(&a, &Keys::default(), &input).is_none());
    }

    #[test]
    fn a_missing_declaration_imports_as_error_and_not_as_a_wrong_type() {
        let mut a = TypeArena::new();
        let mut keys = Keys::default();
        let node = node_interface(&mut a, &mut keys);
        let void = a.void();
        let body = func(&mut a, vec![param("x", node.reference)], void);
        let f = export(
            &a,
            &keys,
            key("f", DeclKind::Function),
            "f",
            DeclKind::Function,
            body,
        );
        let provider = provider_of(&[&f]); // Node is not provided

        let mut b = TypeArena::new();
        let mut importer = Importer::new();
        let ty = importer.import_decl(&mut b, &provider, f.key).unwrap();
        let Type::Function(func) = b.get(ty).clone() else {
            panic!()
        };
        assert_eq!(func.params[0].type_id, b.error());
    }

    #[test]
    fn a_builtin_application_uses_the_shared_builtin_slot() {
        // function p(): Promise<number>
        let mut a = TypeArena::new();
        let slot = a.builtin_slot("Promise");
        let num = a.number();
        let body_of_app = a.alloc(Type::Object(ObjectType::new(vec![prop("value", num)])));
        let app = a.alloc_app(slot, vec![num], body_of_app);
        let body = func(&mut a, vec![], app);
        let keys = Keys {
            slots: vec![(slot, DeclKey::builtin("Promise"), "Promise".into())],
            tparams: vec![],
        };
        let p = export(
            &a,
            &keys,
            key("p", DeclKind::Function),
            "p",
            DeclKind::Function,
            body,
        );

        let mut b = TypeArena::new();
        let mut importer = Importer::new();
        let ty = importer
            .import_decl(&mut b, &provider_of(&[&p]), p.key)
            .unwrap();
        let Type::Function(f) = b.get(ty).clone() else {
            panic!()
        };
        assert_eq!(b.app_parts(f.return_type).map(|(n, _)| n), Some("Promise"));
        // the same slot the checker's own `Promise<...>` uses
        let again = b.builtin_slot("Promise");
        assert_eq!(importer.decl_key(again), Some(DeclKey::builtin("Promise")));
    }

    #[test]
    fn an_unused_generic_signature_parameter_and_default_round_trip() {
        // `<T = string>() => void`: T is absent from both the value parameter list and
        // return type, so only explicit signature metadata can preserve it.
        let mut arena = TypeArena::new();
        let owner = key("factory", DeclKind::Function);
        let id = TypeParameterId::with_file(FileId::ROOT, 17, 0);
        let string = arena.string();
        let void = arena.void();
        let body = arena.alloc(Type::Function(FunctionType {
            params: Vec::new(),
            return_type: void,
            type_params: vec![crate::types::FunctionTypeParam {
                id,
                name: "T".into(),
                constraint: None,
                default: Some(string),
            }],
            is_untyped: false,
        }));
        let keys = Keys {
            slots: Vec::new(),
            tparams: vec![(id, TypeParamKey { owner, index: 0 })],
        };
        let declared = [TypeParamInput {
            id,
            name: "T".into(),
            bound: None,
            default: Some(string),
        }];
        let exported = export_decl(
            &arena,
            &keys,
            &DeclInput {
                key: owner,
                name: "factory",
                kind: DeclKind::Function,
                body,
                type_params: &declared,
            },
        )
        .expect("generic signature has stable parameter keys");

        let provider = provider_of(&[&exported]);
        let mut imported_arena = TypeArena::new();
        let mut importer = Importer::new();
        let imported = importer
            .import_decl(&mut imported_arena, &provider, owner)
            .expect("generic function imports");
        let Type::Function(function) = imported_arena.get(imported).clone() else {
            panic!("imported signature is not a function");
        };
        assert_eq!(function.type_params.len(), 1);
        assert_eq!(function.type_params[0].name, "T");
        assert_eq!(function.type_params[0].constraint, None);
        assert_eq!(
            function.type_params[0].default,
            Some(imported_arena.string())
        );
    }

    #[test]
    fn the_golden_export_does_not_change() {
        // If this fails the portable encoding or the hash changed: bump ENC_VERSION.
        let mut a = TypeArena::new();
        let (f, _) = f_and_node(&mut a);
        assert_eq!(f.sig.as_u128(), GOLDEN_F_SIG, "{:032x}", f.sig.as_u128());
    }

    const GOLDEN_F_SIG: u128 = 0x7f064bd9759f6fef3a369f79740bc0ee;
}
