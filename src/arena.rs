use crate::fxhash::{FxHashMap, FxHasher};
use crate::types::{ObjectType, Type};
use smallvec::SmallVec;
use std::cell::RefCell;
use std::hash::{Hash, Hasher};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TypeId(u32);

// A declaration's place in the arena: an interface, a class or an object-literal alias,
// the kinds that can refer to themselves. The type of such a declaration is a
// Type::Ref to its slot. The Ref is allocated before the declaration's members are
// resolved and is never rewritten, so a member that names the declaration back holds
// its id from the start and nothing has to be patched when the body is finished.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct DeclSlot(u32);

// An intersection whose distribution over unions would be too large to build. tsc reports
// this as TS2590, "Expression produces a union type that is too complex to represent".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TooComplex;

// The product of the union sizes at which alloc_intersection refuses to distribute. This
// is tsc's number (checkCrossProductUnion); LLD 1.13 notes it may be lowered once the
// budgets are measured.
// The built-in generics `builtin_slot` is asked for (see type_annotation::
// resolve_builtin_generic). A unit's resolver asks about each by name.
pub(crate) const BUILTIN_GENERICS: [&str; 2] = ["Promise", "Record"];

const MAX_INTERSECTION_DISTRIBUTION: usize = 100_000;

#[derive(Clone, Copy)]
enum DeclState {
    // The members are still being resolved. The declaration reads as an empty object,
    // which is what an unfinished declaration has always looked like to a caller.
    Resolving,
    // The id of the body, an Object, which is what get() hands out for the Ref. Reached once,
    // through resolve_ref, and that is the one moment an existing id starts reading as
    // something else, which is why it advances `generation`.
    Resolved(TypeId),
    // Resolution gave up. Nothing outside the failed call was given a usable type, and
    // the Ref keeps reading as an empty object.
    Failed,
    // A slot that exists only so a Named or an App has a declaration to print: an alias,
    // an enum, a built-in generic. No Ref points at it.
    NameOnly,
}

struct DeclInfo {
    state: DeclState,
    // How the declaration prints in a message ("Dog"). A generic declaration has none:
    // its body still holds bare type parameters, and each instantiation names itself.
    name: Option<String>,
}

// The (left, right) pairs a recursive comparison is in the middle of. It is a
// SmallVec, not a Vec or a hash set: real comparisons are shallow, so the sixteen
// inline slots almost never spill to the heap and a lookup is a handful of integer
// compares, cheaper than hashing. A pathologically deep type still works, because
// the vector simply moves to the heap.
pub(crate) type PairStack = SmallVec<[(TypeId, TypeId); 16]>;

impl TypeId {
    // The slot number, for a caller that needs to put two ids in a fixed order, for
    // example to store a symmetric answer once instead of once per direction. TypeId is
    // deliberately not Ord: nothing should sort or rank types by where they sit in the
    // arena, so the ordering is opt-in and named for what it is.
    pub(crate) fn index(self) -> u32 {
        self.0
    }
}

// How many slots new() reserves for the primitives (number() .. void()). Those ids
// are shared by every use of the primitive and never get a name of their own: see
// alloc_named.
const FIXED_SLOTS: u32 = 10;

// The 64-bit digest the intern tables are keyed on, in place of the content itself.
//
// Decision: key the tables on a digest and confirm every hit against the slot in
// `types`, which is the source of truth anyway.
// Why: keying on the content would store each interned composite twice (once in
// `types`, once as the map key) and clone it on every miss.
// Cost: a digest collision can never merge two different types, because a hit is
// compared with the slot before it is trusted. It must not cost the type its single id
// either: narrowing compares literals by TypeId, so a literal that missed the table
// would never equal the same literal written elsewhere. Two strings such as
// "variant19" and "variant92" really do share an FxHasher digest, which is why
// `collided` exists.
fn content_hash<T: Hash + ?Sized>(content: &T) -> u64 {
    let mut hasher = FxHasher::default();
    content.hash(&mut hasher);
    hasher.finish()
}

/// Counters for benchmarks and regression reports. They are snapshots of sizes the
/// arena already tracks, so asking for them does not walk the type graph.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TypeArenaStats {
    pub type_count: usize,
    pub capacity: usize,
    pub interned_types: usize,
    pub interned_unions: usize,
    pub named_types: usize,
    pub record_types: usize,
}

pub struct TypeArena {
    types: Vec<Type>,

    // The slots of the built-in generics (`Promise`, `Record`), by name. A built-in has
    // no declaration in the file, so its applications need a slot of their own to hang
    // off, and one per name keeps every `Promise<number>` the same id. Separate from
    // the slots an alias or a user's own `Record` gets, which are never shared.
    builtin_slots: FxHashMap<&'static str, DeclSlot>,

    // Auxiliary index behind alloc(): a digest of the content of every anonymous
    // composite allocated so far -> the one TypeId that holds it. `types` stays the
    // source of truth (a TypeId is still just an index into it); this map only answers
    // "does a slot for exactly this content already exist?" before a new one is
    // pushed. The digest comes from Type's shallow hash (see types.rs), so computing
    // it never walks the arena.
    //
    // A digest match is never trusted on its own: alloc() compares the slot it points
    // at with the incoming type.
    //
    // Problem: two different types can share a 64-bit digest. FxHasher is weak on short
    // strings that differ in a couple of characters, and "variant19" / "variant92" are
    // one such pair. The first answer to a collision was to give the second type a slot
    // of its own on every alloc and leave it out of the table, which cost one missed
    // reuse and could not merge two types that differ. But it also meant the same
    // literal got a new TypeId on every use, and narrowing compares literals by TypeId
    // (it relies on every literal being interned), so `value.kind === "variant92"` found
    // no matching member and the union narrowed to never.
    // Picked: the first type to claim a digest stays here, and any later type that
    // shares it is kept in `collided`, found by comparing against each slot under that
    // digest. Every distinct content has exactly one id again.
    // Cost: a lookup that hits a collided digest scans a handful of ids. Nothing on the
    // path of a type that has its digest to itself changed.
    interned: FxHashMap<u64, TypeId>,

    // Types that share a digest with an earlier, different type, in allocation order.
    // Empty unless a collision has happened.
    collided: FxHashMap<u64, Vec<TypeId>>,

    // The same idea as `interned`, kept apart for unions. A union cannot go through
    // alloc(): its members must be flattened and deduplicated first, and that is
    // alloc_union's job, so the key here is the *finished* member list.
    //
    // Decision: the key is a digest of the members in the order alloc_union produced
    // them (checked against the slot on a hit, like `interned`), not of a sorted list.
    // Why: sorting would merge `A | B` with `B | A`, but the survivor would be
    // whichever was built first, so the member order printed in a diagnostic would
    // depend on build order instead of source order.
    // Cost: reordered unions get separate ids and miss the relation cache;
    // structurally_equal still treats them as one type. A repeated build of the same
    // union (what narrowing does on every condition) still hits.
    //
    // A miss is always safe: two unions that are equal by shape but sit at different
    // ids just get different keys and stay separate.
    interned_unions: FxHashMap<u64, TypeId>,

    // Answers to "does this type mention a type parameter" (see
    // semantic::generics::contains_type_param), one byte per TypeId: 0 not asked
    // yet, 1 no, 2 yes. Every call expression asks this about its callee's
    // parameter and return types, and nearly all of those are plain non-generic
    // types, so remembering the answer across calls turns those walks into a
    // single lookup. It sits behind a RefCell only because the question is asked
    // through &TypeArena; no borrow is held across a call.
    //
    // Cleared by resolve_ref(), the one place what an existing id reads as changes. A
    // Ref that was still unfinished when it was scanned reads as "no parameter", and
    // every type built on top of it inherited that answer, so resolving one drops them
    // all. New slots need no invalidation: an id nobody has asked about is just absent.
    param_scan: RefCell<Vec<u8>>,

    // Answers already given by structurally_equal_cached, one entry per unordered
    // pair (equality is symmetric, so (a, b) and (b, a) share a slot).
    //
    // Only alloc_union asks, and it asks the same questions again whenever narrowing
    // rebuilds a union from the same members. Every answer stays valid until a
    // declaration is resolved: resolve_ref() is the one operation that changes what an
    // existing id reads as, so it empties this table, same as param_scan above.
    equality_cache: FxHashMap<(TypeId, TypeId), bool>,

    // Whether an intersection reduces to never because two members disagree about a
    // discriminant (see intersection_reduces_to_never). Reads member bodies, so an
    // answer from before a declaration was resolved can be wrong afterwards: it is
    // emptied by resolve_ref and clear(), like param_scan above. A RefCell, because the
    // relation code that asks only holds a shared reference to the arena.
    empty_intersections: RefCell<FxHashMap<TypeId, bool>>,

    // Advances whenever an id that already exists changes meaning: resolve_ref()
    // giving a declaration its body, or clear() starting over. Anything that remembers
    // an answer about a TypeId (the relation cache) compares generations instead of
    // relying on each caller to remember an invalidation rule. New ids do not advance
    // it, since an id nobody has asked about has no stale answer.
    generation: u64,

    // The declaration behind every Ref, indexed by DeclSlot. A Ref holds only its slot, so a
    // declaration's state changes here while the id every member holds stays the same (LLD
    // 1.7); that is why a recursive declaration needs no placeholder patched afterwards.
    decls: Vec<DeclInfo>,

    // The Named ids that are enums. An enum prints and relates as a union of its member
    // literals like an alias of one does, but tsc treats it as a primitive and does not
    // elaborate a mismatch below it, so the two have to be told apart.
    enum_types: Vec<TypeId>,

    // What a Ref reads as while its declaration has no body. Kept here so get() can hand
    // out a reference to it instead of building one per call.
    empty_object: Type,
}

impl TypeArena {
    // The ten primitives are allocated in this fixed order so their ids are known
    // constants below (number(), string(), and so on). Any part of the checker that
    // needs "the number type" calls arena.number() directly instead of having to
    // thread a TypeId through from wherever that primitive was first resolved.
    pub fn new() -> Self {
        let mut arena = Self {
            types: Vec::new(),
            builtin_slots: FxHashMap::default(),
            interned: FxHashMap::default(),
            collided: FxHashMap::default(),
            interned_unions: FxHashMap::default(),
            param_scan: RefCell::new(Vec::new()),
            equality_cache: FxHashMap::default(),
            empty_intersections: RefCell::new(FxHashMap::default()),
            generation: 0,
            decls: Vec::new(),
            enum_types: Vec::new(),
            empty_object: Type::Object(ObjectType::new(Vec::new())),
        };

        arena.alloc(Type::Number);
        arena.alloc(Type::String);
        arena.alloc(Type::Boolean);
        arena.alloc(Type::Null);
        arena.alloc(Type::Undefined);
        arena.alloc(Type::Any);
        arena.alloc(Type::Unknown);
        arena.alloc(Type::Error);
        arena.alloc(Type::Never);
        arena.alloc(Type::Void);
        debug_assert_eq!(arena.types.len() as u32, FIXED_SLOTS);
        arena
    }

    // Allocates `ty`, reusing the existing TypeId when an identical anonymous
    // composite was allocated before (hash-consing). Objects, arrays, functions,
    // literals and generic parameters are reused; everything else always gets a
    // new slot:
    //   - the fixed primitives (numbered 0..=9 in new(), never allocated again),
    //   - unions, which are interned by alloc_union itself after it has flattened
    //     and deduplicated them (see interned_unions), and
    //   - declaration references, which alloc_ref pushes directly.
    //
    // Because two calls can return the same TypeId, a TypeId does not identify one
    // allocation. A name is never attached to such an id: an alias or an application
    // wraps the id it names (see alloc_named and alloc_app), so a shared id is never
    // renamed and nothing has to ask for one of its own.
    //
    // A GenericParameter is not at risk: its key includes its TypeParameterId,
    // so a `T` from one declaration never matches a `T` from another.
    pub fn alloc(&mut self, ty: Type) -> TypeId {
        if !Self::is_internable(&ty) {
            return self.push(ty);
        }
        let digest = content_hash(&ty);
        if let Some(&existing) = self.interned.get(&digest) {
            if self.types[existing.0 as usize] == ty {
                return existing;
            }
            // Same digest, different content (see the note on `interned`): look among
            // the others that share it, and add this one if it is new.
            let already = self.collided.get(&digest).and_then(|ids| {
                ids.iter()
                    .copied()
                    .find(|&id| self.types[id.0 as usize] == ty)
            });
            if let Some(found) = already {
                return found;
            }
            let id = self.push(ty);
            self.collided.entry(digest).or_default().push(id);
            return id;
        }
        let id = self.push(ty);
        self.interned.insert(digest, id);
        id
    }

    // A slot that exists to give a Named or an App a declaration to print, with no body
    // behind it. An alias and an enum each get one of their own, so two declarations
    // never share a name by accident.
    pub fn alloc_name(&mut self, name: impl Into<String>) -> DeclSlot {
        let slot = DeclSlot(self.decls.len() as u32);
        self.decls.push(DeclInfo {
            state: DeclState::NameOnly,
            name: Some(name.into()),
        });
        slot
    }

    // The one slot of a built-in generic such as `Promise`. Asked for again, it is the
    // same slot, which is what makes every `Promise<number>` the same id.
    pub fn builtin_slot(&mut self, name: &'static str) -> DeclSlot {
        if let Some(&slot) = self.builtin_slots.get(name) {
            return slot;
        }
        let slot = self.alloc_name(name);
        self.builtin_slots.insert(name, slot);
        slot
    }

    // `inner` under the name of `slot`: an alias (`type Scores = number[]`) or an enum.
    // The wrapper is what carries the name, so `inner` stays the id everything else
    // shares and is never renamed.
    //
    // A fixed primitive is returned as it is. tsc prints `type Age = number` as the
    // primitive itself, and the one slot every `number` shares must not be wrapped.
    pub fn alloc_named(&mut self, slot: DeclSlot, inner: TypeId) -> TypeId {
        if inner.0 < FIXED_SLOTS {
            return inner;
        }
        self.alloc(Type::Named(slot, inner))
    }

    // A generic declaration applied to `args`, with `body` as what that application is.
    // The same declaration, arguments and body give the same id, so an instantiation
    // can be remembered by the id this returns and handed out as it is.
    pub fn alloc_app(&mut self, slot: DeclSlot, args: Vec<TypeId>, body: TypeId) -> TypeId {
        self.alloc(Type::App(slot, args, body))
    }

    // A new slot that never enters the intern table, so the id is this call's own even
    // when identical content was allocated before. Tests use it to get distinct ids for
    // content that alloc() would give one id.
    #[cfg(test)]
    pub(crate) fn alloc_fresh(&mut self, ty: Type) -> TypeId {
        self.push(ty)
    }

    fn push(&mut self, ty: Type) -> TypeId {
        self.types.push(ty);
        TypeId((self.types.len() - 1) as u32)
    }

    // The kinds that share one id by content. A primitive already has a fixed slot, a union
    // is interned by its own table (interned_unions) and a Ref is pushed directly (see
    // alloc_ref), so none of them goes through the digest table.
    fn is_internable(ty: &Type) -> bool {
        matches!(
            ty,
            Type::Object(_)
                | Type::Array(_)
                | Type::Function(_)
                | Type::StringLiteral(_)
                | Type::NumberLiteral(_)
                | Type::BooleanLiteral(_)
                | Type::GenericParameter(..)
                | Type::Named(..)
                | Type::App(..)
                | Type::Intersection(..)
        )
    }

    // Whether `id` is the shared slot the intern table hands out for its content.
    // Checked by looking the content up rather than tracking a flag per slot: a Ref or
    // an alloc_fresh id can hold (or read as) content identical to an interned one, but
    // the table maps that content to a different id, so it correctly reports false. A
    // union is answered from its own table.
    #[cfg(test)]
    fn is_interned(&self, id: TypeId) -> bool {
        let ty = self.get(id);
        if let Type::Union(members) = ty {
            return self.interned_unions.get(&content_hash(members)) == Some(&id);
        }
        if !Self::is_internable(ty) {
            return false;
        }
        let digest = content_hash(ty);
        self.interned.get(&digest) == Some(&id)
            || self
                .collided
                .get(&digest)
                .is_some_and(|ids| ids.contains(&id))
    }

    // Flattens nested unions and drops never members, since never contributes
    // nothing to what a union can hold. A one-member result collapses to that
    // member directly rather than a redundant single-member Type::Union, so
    // callers get a plain type back instead of having to unwrap a trivial union.
    pub fn alloc_union(&mut self, members: Vec<TypeId>) -> TypeId {
        let mut flat: Vec<TypeId> = Vec::with_capacity(members.len());
        let mut queue = members;
        while let Some(id) = queue.pop() {
            match self.get(id) {
                Type::Union(nested) => {
                    queue.extend(nested.iter().copied());
                    continue;
                }
                Type::Never => continue,
                _ => {}
            }
            // Plain structurally_equal, not structurally_equal_cached. This loop is
            // the only place the cached form was tried, and a diagnostic-count
            // regression appeared there; the root cause was not established. The
            // uncached form is the one known to keep the counts stable, so it stays
            // until the cause is found. structurally_equal_cached is kept and tested
            // for reconnecting.
            let already_present = flat
                .iter()
                .any(|&existing| self.structurally_equal(existing, id));
            if !already_present {
                flat.push(id);
            }
        }

        match flat.len() {
            0 => self.never(),
            1 => flat[0],
            // Not alloc(): that table is keyed on a digest of a Type, and a union's
            // identity is its finished member list, which only exists here. Reusing
            // the id is what lets the subtype cache, keyed on (TypeId, TypeId), hit
            // when narrowing rebuilds the same union.
            _ => {
                let digest = content_hash(&flat);
                if let Some(&existing) = self.interned_unions.get(&digest) {
                    if matches!(&self.types[existing.0 as usize], Type::Union(m) if *m == flat) {
                        return existing;
                    }
                    return self.push(Type::Union(flat));
                }
                let id = self.push(Type::Union(flat));
                self.interned_unions.insert(digest, id);
                id
            }
        }
    }

    // An intersection in its canonical form (LLD 1.13), or TooComplex when distributing it
    // over its unions would not fit. Every intersection is built here, so the node always
    // arrives reduced: substitution and narrowing go through this too, which is how
    // `T & number` with `T := string` becomes `never`.
    //
    // Everything below reads only what an id and its Named/App wrappers show, never the
    // body behind a Ref. Construction runs while declarations are still being resolved,
    // when a Ref reads as an empty object, and a decision that read it would be written
    // into the intern table under an id nothing invalidates. What needs a body is
    // answered later and on demand, see intersection_reduces_to_never.
    pub fn alloc_intersection(&mut self, members: Vec<TypeId>) -> Result<TypeId, TooComplex> {
        // `never` ends it, and a nested intersection is spread into its members in place.
        let mut flat: Vec<TypeId> = Vec::with_capacity(members.len());
        for member in members {
            match self.shallow(member) {
                Type::Never => return Ok(self.never()),
                Type::Intersection(inner) => flat.extend(inner.iter().copied()),
                _ => flat.push(member),
            }
        }

        // `unknown` adds nothing. The error type then `any` absorb the rest, the error
        // type first so a failure that was already reported stays the sentinel.
        flat.retain(|&member| !matches!(self.shallow(member), Type::Unknown));
        if flat
            .iter()
            .any(|&member| matches!(self.shallow(member), Type::Error))
        {
            return Ok(self.error());
        }
        if flat
            .iter()
            .any(|&member| matches!(self.shallow(member), Type::Any))
        {
            return Ok(self.any());
        }

        if self.primitives_cancel(&mut flat) {
            return Ok(self.never());
        }

        // A union among the members is distributed: `(A | B) & C` is `A & C | B & C`, in
        // written order. Each choice is built by this same function, so what is left to
        // reduce after one member is replaced gets reduced.
        if let Some(position) = flat
            .iter()
            .position(|&member| matches!(self.shallow(member), Type::Union(_)))
        {
            let size = flat
                .iter()
                .try_fold(1usize, |size, &member| match self.shallow(member) {
                    Type::Union(alternatives) => size.checked_mul(alternatives.len()),
                    _ => Some(size),
                });
            if size.is_none_or(|size| size >= MAX_INTERSECTION_DISTRIBUTION) {
                return Err(TooComplex);
            }
            let Type::Union(alternatives) = self.shallow(flat[position]) else {
                unreachable!("position found a union");
            };
            let alternatives = alternatives.clone();
            let mut results = Vec::with_capacity(alternatives.len());
            for alternative in alternatives {
                let mut choice = flat.clone();
                choice[position] = alternative;
                results.push(self.alloc_intersection(choice)?);
            }
            // alloc_union keeps its members in the reverse of the order it is given, so
            // handing it the results as they came would flip the union's stored order,
            // and `("a" | "b") & string` would be a different id from `"a" | "b"` though
            // it is the same type. Reversed, the result keeps the order of the union it
            // was distributed from.
            results.reverse();
            return Ok(self.alloc_union(results));
        }

        // An anonymous `{}` says nothing next to another object-like member: `A & {}` is
        // `A`. Next to a primitive or a type parameter it still means "not null", so
        // `string & {}` and `T & {}` stay as they are.
        if flat.len() > 1
            && flat.iter().any(|&member| {
                self.is_object_like(member) && !self.is_anonymous_empty_object(member)
            })
        {
            flat.retain(|&member| !self.is_anonymous_empty_object(member));
        }

        // By id, keeping the first, and never structurally: structural equality reads
        // bodies.
        let mut unique: Vec<TypeId> = Vec::with_capacity(flat.len());
        for member in flat {
            if !unique.contains(&member) {
                unique.push(member);
            }
        }

        Ok(match unique.len() {
            0 => self.unknown(),
            1 => unique[0],
            _ => self.alloc(Type::Intersection(unique)),
        })
    }

    // Primitive and literal members that cancel each other, or collapse. True when the
    // whole intersection is empty. Otherwise `flat` is left holding the more specific
    // member of any pair that overlaps: `string & "a"` keeps `"a"`, and `void & undefined`
    // keeps `undefined`.
    //
    // An object-like member never cancels a primitive, so a branded primitive such as
    // `string & { __brand: "id" }` stays an intersection. `null` and `undefined` are the
    // exception: nothing but themselves, `void` (for undefined) and a type parameter,
    // which could be anything, shares a value with them. A union is left for the
    // distribution step, since `null & (null | A)` is `null`.
    fn primitives_cancel(&self, flat: &mut Vec<TypeId>) -> bool {
        use crate::subtyping::{PrimitiveDomain, is_disjoint, primitive_domain};

        for (index, &a) in flat.iter().enumerate() {
            for &b in &flat[index + 1..] {
                let both_primitive = primitive_domain(self.shallow(a)).is_some()
                    && primitive_domain(self.shallow(b)).is_some();
                if both_primitive && is_disjoint(self, a, b) {
                    return true;
                }
            }
        }

        let only_themselves = |member: &Type, allowed: &[fn(&Type) -> bool]| {
            matches!(member, Type::GenericParameter(..) | Type::Union(_))
                || allowed.iter().any(|allows| allows(member))
        };
        for &member in flat.iter() {
            let others_ok = match self.shallow(member) {
                Type::Null => flat.iter().all(|&other| {
                    only_themselves(self.shallow(other), &[|ty| matches!(ty, Type::Null)])
                }),
                Type::Undefined => flat.iter().all(|&other| {
                    only_themselves(
                        self.shallow(other),
                        &[|ty| matches!(ty, Type::Undefined | Type::Void)],
                    )
                }),
                _ => true,
            };
            if !others_ok {
                return true;
            }
        }

        // Of two members in the same domain the literal wins over its primitive, and
        // `undefined` over `void`.
        let literal_domains: Vec<PrimitiveDomain> = flat
            .iter()
            .filter(|&&member| {
                matches!(
                    self.shallow(member),
                    Type::StringLiteral(_) | Type::NumberLiteral(_) | Type::BooleanLiteral(_)
                )
            })
            .filter_map(|&member| primitive_domain(self.shallow(member)))
            .collect();
        let has_undefined = flat
            .iter()
            .any(|&member| matches!(self.shallow(member), Type::Undefined));
        flat.retain(|&member| {
            let ty = self.shallow(member);
            let is_plain_primitive = matches!(ty, Type::String | Type::Number | Type::Boolean);
            let covered =
                primitive_domain(ty).is_some_and(|domain| literal_domains.contains(&domain));
            !(is_plain_primitive && covered) && !(has_undefined && matches!(ty, Type::Void))
        });
        false
    }

    // Looks through Named and App wrappers and stops at a Ref. That is the whole of what
    // alloc_intersection may know about a member: a wrapper's contents exist when the
    // wrapper does, and a Ref's body may not exist yet.
    fn shallow(&self, id: TypeId) -> &Type {
        let mut current = id;
        loop {
            match &self.types[current.0 as usize] {
                Type::Named(_, inner) => current = *inner,
                Type::App(_, _, body) => current = *body,
                other => return other,
            }
        }
    }

    // Whether a member is a function, an array, an object or a declaration: the kinds a
    // primitive can be branded with and that an empty `{}` adds nothing to. A type
    // parameter is not, since `T & {}` has to stay.
    fn is_object_like(&self, id: TypeId) -> bool {
        matches!(
            self.shallow(id),
            Type::Object(_) | Type::Function(_) | Type::Array(_) | Type::Ref(_)
        )
    }

    // The anonymous `{}` itself, read from the member's own node. A Promise<number> is an
    // App over an empty body but is not the empty object, so this does not look through
    // wrappers.
    fn is_anonymous_empty_object(&self, id: TypeId) -> bool {
        matches!(&self.types[id.0 as usize], Type::Object(object) if object.properties.is_empty())
    }

    // Whether an intersection of objects is empty because two members give the same
    // property types that cannot meet and one of them is a literal: `{ kind: "a" } & { kind:
    // "b" }`. tsc keeps such an intersection as it is and reduces it when something asks,
    // which is why it prints as `never` while still being an intersection. So does this:
    // the answer reads member bodies, which alloc_intersection must not, and it is
    // cached per id until a declaration is resolved (see empty_intersections).
    //
    // A property whose types are not literals is not a discriminant: `{ x: number } & {
    // x: string }` is not empty, it has an `x` of type never.
    pub(crate) fn intersection_reduces_to_never(&self, id: TypeId) -> bool {
        let Type::Intersection(members) = self.get(id) else {
            return false;
        };
        if let Some(&known) = self.empty_intersections.borrow().get(&id) {
            return known;
        }

        let objects: Vec<&crate::types::ObjectType> = members
            .iter()
            .filter_map(|&member| match self.get(member) {
                Type::Object(object) => Some(object),
                _ => None,
            })
            .collect();

        let is_unit = |type_id: TypeId| match self.get(type_id) {
            Type::StringLiteral(_) | Type::NumberLiteral(_) | Type::BooleanLiteral(_) => true,
            Type::Union(alternatives) => alternatives.iter().any(|&alternative| {
                matches!(
                    self.get(alternative),
                    Type::StringLiteral(_) | Type::NumberLiteral(_) | Type::BooleanLiteral(_)
                )
            }),
            _ => false,
        };

        let mut empty = false;
        'pairs: for (index, left) in objects.iter().enumerate() {
            for right in &objects[index + 1..] {
                for property in left.properties.iter() {
                    let Some(other) = right.properties.iter().find(|p| p.name == property.name)
                    else {
                        continue;
                    };
                    if crate::subtyping::is_disjoint(self, property.type_id, other.type_id)
                        && (is_unit(property.type_id) || is_unit(other.type_id))
                    {
                        empty = true;
                        break 'pairs;
                    }
                }
            }
        }

        self.empty_intersections.borrow_mut().insert(id, empty);
        empty
    }

    // The type behind `id`. A declaration's Ref reads as its body once the body exists
    // and as an empty object before; a Named reads as what it names and an App as its
    // body. So callers match on the structural type and never see a wrapper. Code that
    // has to tell two declarations apart (to print a name, to check a slot) asks about
    // the id it was given, not about what this returns.
    //
    // Terminates because a wrapper is allocated after the id it points at, and a Ref
    // resolves to a body, never to another Ref.
    pub fn get(&self, id: TypeId) -> &Type {
        let mut current = id;
        loop {
            match &self.types[current.0 as usize] {
                Type::Ref(slot) => match self.decls[slot.0 as usize].state {
                    DeclState::Resolved(body) => current = body,
                    DeclState::Resolving | DeclState::Failed | DeclState::NameOnly => {
                        return &self.empty_object;
                    }
                },
                Type::Named(_, inner) => current = *inner,
                Type::App(_, _, body) => current = *body,
                ty => return ty,
            }
        }
    }

    // The type stored under `id` exactly as written: a Ref, a Named and an App are
    // returned as themselves, not looked through like get() does. Stable hashing needs
    // this, because a declaration's identity is the wrapper.
    pub(crate) fn raw(&self, id: TypeId) -> &Type {
        &self.types[id.0 as usize]
    }

    // None for an id that is not a declaration, and also for one the arena does not hold
    // (a stale id from before clear()), so that asking for a name stays harmless.
    pub(crate) fn slot_of(&self, id: TypeId) -> Option<DeclSlot> {
        match self.types.get(id.0 as usize) {
            Some(Type::Ref(slot)) => Some(*slot),
            _ => None,
        }
    }

    // The slot and body of a declaration as the namespace hands it out: a resolved Ref
    // (interface, class, type-literal alias) or a Named (alias). None for any other id,
    // including a Ref that has no body yet. Exporting needs the slot, to tell the unit's
    // resolver which declaration it is, and the body, which is what gets exported.
    pub(crate) fn decl_parts(&self, id: TypeId) -> Option<(DeclSlot, TypeId)> {
        match self.types.get(id.0 as usize)? {
            Type::Ref(slot) => match self.decls[slot.0 as usize].state {
                DeclState::Resolved(body) => Some((*slot, body)),
                _ => None,
            },
            Type::Named(slot, inner) => Some((*slot, *inner)),
            _ => None,
        }
    }

    // The slot of a built-in generic if one has been made in this arena, without making
    // it. Looked up by name and never by walking the table, so the answer cannot depend
    // on hash order (LLD 8.2).
    pub(crate) fn existing_builtin_slot(&self, name: &str) -> Option<DeclSlot> {
        self.builtin_slots.get(name).copied()
    }

    // Names the declaration behind `reference`. Called once its body exists, for a
    // declaration that is not generic: a generic's body still holds bare type
    // parameters, and each application of it prints its own name. A later call replaces
    // the earlier name.
    pub fn name_ref(&mut self, reference: TypeId, name: impl Into<String>) {
        debug_assert!(
            self.slot_of(reference).is_some(),
            "name_ref needs an id from alloc_ref"
        );
        if let Some(slot) = self.slot_of(reference) {
            self.decls[slot.0 as usize].name = Some(name.into());
        }
    }

    // The name `id` prints under when it has one that does not depend on anything else:
    // a named declaration or an alias. An application is not covered, its name comes
    // from its arguments (see app_parts), and is_named answers for all of them.
    pub fn display_name(&self, type_id: TypeId) -> Option<&str> {
        let slot = match self.types.get(type_id.0 as usize)? {
            Type::Ref(slot) | Type::Named(slot, _) => *slot,
            _ => return None,
        };
        self.decls[slot.0 as usize].name.as_deref()
    }

    // The declaration's name and the arguments of an application, for display to write
    // out as `Box<Dog>`.
    pub fn app_parts(&self, type_id: TypeId) -> Option<(&str, &[TypeId])> {
        match self.types.get(type_id.0 as usize)? {
            Type::App(slot, args, _) => {
                let name = self.decls[slot.0 as usize].name.as_deref()?;
                Some((name, args.as_slice()))
            }
            _ => None,
        }
    }

    /// The two types an enum stands for, from its resolved members: the type position (the
    /// union of the member literals, under the enum's name and marked as an enum) and the
    /// value position (an object of the members, which prints as `typeof Name`). One
    /// builder for a local declaration and for one imported from another file, so the two
    /// cannot drift apart.
    pub fn build_enum(&mut self, name: &str, members: Vec<(String, TypeId)>) -> (TypeId, TypeId) {
        let member_types: Vec<TypeId> = members.iter().map(|(_, type_id)| *type_id).collect();
        let union = self.alloc_union(member_types);
        // Prints as `Weird`, not the union of its member literals, in a message. The name
        // goes on a wrapper around the union: a one-member enum collapses to its member's
        // literal type, an id every identical literal shares, and that id stays unnamed.
        let type_name = self.alloc_name(name.to_string());
        let type_position = self.alloc_named(type_name, union);
        self.mark_enum(type_position);

        // A member read off the enum (`Color.Red`) prints as `Color` in a message, the way
        // tsc widens an enum literal when it is the source of a mismatch, so each member's
        // type carries the enum's name. The wrapper is transparent to every relation.
        let enum_name = self.alloc_name(name.to_string());
        let properties = members
            .into_iter()
            .map(|(member, type_id)| crate::types::PropertyEntry {
                name: member.into(),
                type_id: self.alloc_named(enum_name, type_id),
                optional: false,
                is_method: false,
            })
            .collect();
        // ObjectType::new sorts by name, which the merge-join in
        // subtyping::object_is_subtype relies on; members arrive in declaration order.
        let value_type = self.alloc(Type::Object(crate::types::ObjectType::new(properties)));
        // The enum object itself prints as `typeof Color`.
        let object_name = self.alloc_name(format!("typeof {name}"));
        let value_type = self.alloc_named(object_name, value_type);
        (type_position, value_type)
    }

    pub fn mark_enum(&mut self, type_id: TypeId) {
        self.enum_types.push(type_id);
    }

    pub fn is_enum(&self, type_id: TypeId) -> bool {
        self.enum_types.contains(&type_id)
    }

    // Whether `id` prints under a name and not as its structure.
    pub fn is_named(&self, type_id: TypeId) -> bool {
        self.display_name(type_id).is_some() || self.app_parts(type_id).is_some()
    }

    // Whether `type_id` is a `Promise<T>`: an App of the built-in Promise slot.
    pub fn is_promise(&self, type_id: TypeId) -> bool {
        let Some(&promise) = self.builtin_slots.get("Promise") else {
            return false;
        };
        matches!(
            self.types.get(type_id.0 as usize),
            Some(Type::App(slot, _, _)) if *slot == promise
        )
    }

    // The value type V of a `Record<K, V>`: any key reads as V. Record has no real index
    // signature here, so this is the narrower stand-in the member access code uses (see
    // type_annotation::resolve_builtin_generic), and it does not model K at all.
    pub fn record_value_type(&self, type_id: TypeId) -> Option<TypeId> {
        let record = *self.builtin_slots.get("Record")?;
        match self.types.get(type_id.0 as usize)? {
            Type::App(slot, args, _) if *slot == record => args.get(1).copied(),
            _ => None,
        }
    }

    // Allocates the Ref for a declaration that is about to be resolved and hands back its
    // id. This is what lets an interface, a class or an object-literal alias refer to
    // itself through a property (a linked list's `next: Node | null`) before its own
    // body exists: the self-reference resolves to this id by identity, and resolve_ref
    // later gives the same id its body (see namespace::resolve for how it is used).
    //
    // Pushed directly, never through alloc(): every declaration needs an id of its own,
    // and two of them must never be reused for one another. Unlike the empty object
    // this replaces, the Ref is never rewritten. The body is a separate type, so it can
    // be interned and shared like any other, and what a Ref points at is the only
    // thing that changes.
    pub fn alloc_ref(&mut self) -> TypeId {
        let slot = DeclSlot(self.decls.len() as u32);
        self.decls.push(DeclInfo {
            state: DeclState::Resolving,
            name: None,
        });
        self.push(Type::Ref(slot))
    }

    // Gives the declaration behind `reference` its body. Until this runs the Ref reads
    // as an empty object, so every answer remembered about it, or about anything built
    // on top of it, was an answer about that empty object: the generation moves and
    // the two caches that could hold one are emptied.
    pub fn resolve_ref(&mut self, reference: TypeId, body: TypeId) {
        let slot = self.slot_of(reference);
        debug_assert!(slot.is_some(), "resolve_ref needs an id from alloc_ref");
        let Some(slot) = slot else {
            return;
        };
        debug_assert!(
            self.slot_of(body).is_none(),
            "a body must be the type itself, not another declaration's Ref"
        );
        self.decls[slot.0 as usize].state = DeclState::Resolved(body);
        self.generation = self.generation.wrapping_add(1);
        self.param_scan.get_mut().clear();
        // Two unfinished declarations compare equal (both read as empty objects), and
        // that answer stops being true the moment either one is resolved.
        self.equality_cache.clear();
        self.empty_intersections.get_mut().clear();
    }

    // The declaration could not be resolved. Its Ref keeps reading as an empty object.
    // Nothing is invalidated: no answer about it ever rested on a body.
    pub fn fail_ref(&mut self, reference: TypeId) {
        if let Some(slot) = self.slot_of(reference) {
            self.decls[slot.0 as usize].state = DeclState::Failed;
        }
    }

    // The remembered answer for `id`, if contains_type_param has settled it.
    pub(crate) fn cached_mentions_type_param(&self, id: TypeId) -> Option<bool> {
        match self.param_scan.borrow().get(id.0 as usize) {
            Some(1) => Some(false),
            Some(2) => Some(true),
            _ => None,
        }
    }

    pub(crate) fn remember_mentions_type_param(&self, id: TypeId, found: bool) {
        let mut cache = self.param_scan.borrow_mut();
        let index = id.0 as usize;
        if cache.len() <= index {
            // Grow to the arena's current size in one step rather than one slot
            // at a time as new ids get asked about.
            let wanted = self.types.len().max(index + 1);
            cache.resize(wanted, 0);
        }
        cache[index] = if found { 2 } else { 1 };
    }

    // structurally_equal with a memo, for callers that already hold `&mut self` and ask
    // the same pair more than once.
    //
    // Each answer is computed by a fresh top-level structurally_equal, never taken from
    // inside a walk. Answers reached mid-walk rest on the assumption that a pair
    // already being compared is equal, which only holds for that one comparison, so
    // storing them would be wrong for the next caller.
    //
    // Not called from production code: alloc_union uses the plain structurally_equal
    // (see the note there). Kept and tested so it can be reconnected, so dead_code is
    // allowed outside test builds instead of deleting it.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn structurally_equal_cached(&mut self, a: TypeId, b: TypeId) -> bool {
        if a == b {
            return true;
        }
        let key = if a.0 <= b.0 { (a, b) } else { (b, a) };
        if let Some(&known) = self.equality_cache.get(&key) {
            return known;
        }
        let equal = self.structurally_equal(a, b);
        self.equality_cache.insert(key, equal);
        equal
    }

    // Whether two types are the same type by shape, not by arena slot.
    //
    // The derived PartialEq on Type cannot answer this: it compares a composite's
    // TypeId fields by raw slot, so `{ a: { b: number } }` written at two sites,
    // whose inner objects sit at different slots, compare as unequal even though
    // nothing distinguishes them. This follows TypeIds through the arena instead.
    //
    // Terminates even on recursive types. The graph is acyclic except through a
    // declaration's Ref: alloc_ref hands out an id, the members are resolved
    // against it, and resolve_ref then gives it its body, so `interface N { next: N |
    // null }` contains its own id. Comparing two such types (say two interfaces
    // with the same recursive shape) would otherwise go N -> M -> N -> M forever.
    // Only an Object can close a cycle, so an object pair already being compared
    // further up the stack is assumed equal (the coinductive rule, the same one
    // subtyping uses); any real difference elsewhere in the type still shows up.
    //
    // Identity is the one thing shape must not override. A GenericParameter is
    // equal only to the same declared parameter (its TypeParameterId), never to
    // another `T` that merely has the same name or bound.
    pub fn structurally_equal(&self, a: TypeId, b: TypeId) -> bool {
        self.structurally_equal_inner(a, b, &mut PairStack::new())
    }

    fn structurally_equal_inner(&self, a: TypeId, b: TypeId, seen: &mut PairStack) -> bool {
        if a == b {
            return true;
        }

        match (self.get(a), self.get(b)) {
            (Type::Number, Type::Number)
            | (Type::String, Type::String)
            | (Type::Boolean, Type::Boolean)
            | (Type::Null, Type::Null)
            | (Type::Undefined, Type::Undefined)
            | (Type::Any, Type::Any)
            | (Type::Unknown, Type::Unknown)
            | (Type::Error, Type::Error)
            | (Type::Never, Type::Never)
            | (Type::Void, Type::Void) => true,

            (Type::StringLiteral(x), Type::StringLiteral(y)) => x == y,
            // Bit pattern, the same rule Type's own Eq/Hash use, so an interned
            // literal and this comparison never disagree about 0.0 and -0.0.
            (Type::NumberLiteral(x), Type::NumberLiteral(y)) => x.to_bits() == y.to_bits(),
            (Type::BooleanLiteral(x), Type::BooleanLiteral(y)) => x == y,

            (Type::Array(x), Type::Array(y)) => self.structurally_equal_inner(*x, *y, seen),

            (Type::Function(f), Type::Function(g)) => {
                f.is_untyped == g.is_untyped
                    && f.type_params == g.type_params
                    && f.params.len() == g.params.len()
                    && f.params.iter().zip(&g.params).all(|(p, q)| {
                        p.optional == q.optional
                            && p.rest == q.rest
                            && self.structurally_equal_inner(p.type_id, q.type_id, seen)
                    })
                    && self.structurally_equal_inner(f.return_type, g.return_type, seen)
            }

            // Pairwise, which is only right because ObjectType keeps its
            // properties sorted by name. is_method is part of the shape: it is in
            // the intern key, and subtyping treats a method bivariantly but a
            // function-valued property contravariantly, so merging the two in a
            // union would change what the union accepts.
            (Type::Object(x), Type::Object(y)) => {
                if x.properties.len() != y.properties.len() {
                    return false;
                }
                // Re-entering a pair already being compared is the cycle a
                // placeholder creates; see the note above the function.
                let pair = (a, b);
                if seen.contains(&pair) {
                    return true;
                }
                seen.push(pair);
                let equal = x.properties.iter().zip(y.properties.iter()).all(|(p, q)| {
                    p.name == q.name
                        && p.optional == q.optional
                        && p.is_method == q.is_method
                        && self.structurally_equal_inner(p.type_id, q.type_id, seen)
                });
                seen.pop();
                equal
            }

            // A union is a set: member order carries no meaning. Union members are
            // already deduplicated by alloc_union, so equal length plus every
            // member of one having a match in the other is set equality.
            (Type::Union(xs), Type::Union(ys)) => {
                xs.len() == ys.len()
                    && xs.iter().all(|&x| {
                        ys.iter()
                            .any(|&y| self.structurally_equal_inner(x, y, seen))
                    })
            }

            // As a relation an intersection is a set too: `A & B` and `B & A` are two ids
            // but one type. Only the order of call signatures tells them apart, and that
            // is not a question of equality. Members are deduplicated by alloc_intersection.
            (Type::Intersection(xs), Type::Intersection(ys)) => {
                xs.len() == ys.len()
                    && xs.iter().all(|&x| {
                        ys.iter()
                            .any(|&y| self.structurally_equal_inner(x, y, seen))
                    })
            }

            (Type::GenericParameter(x, _, _), Type::GenericParameter(y, _, _)) => x == y,

            _ => false,
        }
    }

    // Starts over for the next file in a session. Truncating instead of replacing the
    // vectors keeps their allocations, and the ten primitive slots survive untouched,
    // so number() and friends return the same ids as in a fresh arena. Every side
    // table is cleared with the type graph: a table that survived would let the next
    // file see a stale display name, a stale intern entry or a stale cached answer.
    // The generation moves forward so a relation cache built before the reset
    // discards its answers on its next use.
    pub fn clear(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.types.truncate(FIXED_SLOTS as usize);
        self.decls.clear();
        self.enum_types.clear();
        self.builtin_slots.clear();
        self.interned.clear();
        self.collided.clear();
        self.interned_unions.clear();
        self.param_scan.get_mut().clear();
        self.equality_cache.clear();
        self.empty_intersections.get_mut().clear();
        debug_assert_eq!(self.types.len() as u32, FIXED_SLOTS);
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn len(&self) -> usize {
        self.types.len()
    }

    /// Sizes for benchmarks and heap-regression reports. Capacity is included on
    /// purpose: after a large file a reused arena keeps its allocation, so the type
    /// count alone understates what it holds. The maps report entry counts, not
    /// allocator capacity, which keeps this a semantic measure and not a detail of the
    /// hash table.
    #[allow(
        clippy::disallowed_methods,
        reason = "a sum does not depend on iteration order"
    )]
    pub fn stats(&self) -> TypeArenaStats {
        TypeArenaStats {
            type_count: self.types.len(),
            capacity: self.types.capacity(),
            interned_types: self.interned.len()
                + self.collided.values().map(Vec::len).sum::<usize>(),
            interned_unions: self.interned_unions.len(),
            named_types: (0..self.types.len())
                .filter(|&index| self.is_named(TypeId(index as u32)))
                .count(),
            record_types: (0..self.types.len())
                .filter(|&index| self.record_value_type(TypeId(index as u32)).is_some())
                .count(),
        }
    }

    // Fixed slots assigned in new(). These never change for the lifetime of an arena.
    pub fn number(&self) -> TypeId {
        TypeId(0)
    }
    pub fn string(&self) -> TypeId {
        TypeId(1)
    }
    pub fn boolean(&self) -> TypeId {
        TypeId(2)
    }
    pub fn null(&self) -> TypeId {
        TypeId(3)
    }
    pub fn undefined(&self) -> TypeId {
        TypeId(4)
    }
    pub fn any(&self) -> TypeId {
        TypeId(5)
    }
    pub fn unknown(&self) -> TypeId {
        TypeId(6)
    }

    pub fn error(&self) -> TypeId {
        TypeId(7)
    }

    pub fn never(&self) -> TypeId {
        TypeId(8)
    }

    pub fn void(&self) -> TypeId {
        TypeId(9)
    }
}

impl Default for TypeArena {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FunctionType, ObjectType, Param, PropertyEntry, TypeParameterId};

    fn property(name: &str, type_id: TypeId, optional: bool) -> PropertyEntry {
        PropertyEntry {
            name: name.into(),
            type_id,
            optional,
            is_method: false,
        }
    }

    // Built with alloc_fresh, not alloc: these tests exercise structurally_equal on
    // identical shapes that sit in *different* slots, which alloc's hash-consing
    // would otherwise collapse into one before the comparison could matter. The
    // consing behavior itself is covered by the alloc_* tests further down.
    fn object(arena: &mut TypeArena, properties: Vec<PropertyEntry>) -> TypeId {
        arena.alloc_fresh(Type::Object(ObjectType::new(properties)))
    }

    // `{ <name>: <type_id> }` through the consing alloc(), for the tests that are
    // about alloc() itself rather than about structurally_equal.
    fn shared_object(arena: &mut TypeArena, name: &str, type_id: TypeId, optional: bool) -> TypeId {
        arena.alloc(Type::Object(ObjectType::new(vec![property(
            name, type_id, optional,
        )])))
    }

    // `{ a: { b: number } }`, built from scratch each call so every call lands in
    // fresh arena slots.
    fn nested(arena: &mut TypeArena) -> TypeId {
        let number = arena.number();
        let inner = object(arena, vec![property("b", number, false)]);
        object(arena, vec![property("a", inner, false)])
    }

    #[test]
    fn derived_equality_cannot_see_through_a_slot_but_structural_equality_can() {
        let mut arena = TypeArena::new();
        let first = nested(&mut arena);
        let second = nested(&mut arena);

        assert_ne!(first, second, "separately allocated, so different slots");
        assert_ne!(
            arena.get(first),
            arena.get(second),
            "this is the gap: the derived == compares the inner object's slot"
        );
        assert!(arena.structurally_equal(first, second));
    }

    #[test]
    fn union_collapses_structurally_identical_composites() {
        let mut arena = TypeArena::new();
        let first = nested(&mut arena);
        let second = nested(&mut arena);

        let union = arena.alloc_union(vec![first, second]);

        assert!(
            matches!(arena.get(union), Type::Object(_)),
            "two identical shapes must collapse to one member, not stay a two-member union"
        );
    }

    // The point of interning unions: a repeated build must give back the same id, or
    // the subtype cache keyed on (TypeId, TypeId) can never hit on it.
    #[test]
    fn building_the_same_union_twice_reuses_its_id() {
        let mut arena = TypeArena::new();
        let a = arena.alloc(Type::StringLiteral("a".to_string()));
        let b = arena.alloc(Type::StringLiteral("b".to_string()));

        let first = arena.alloc_union(vec![a, b]);
        let second = arena.alloc_union(vec![a, b]);

        assert_eq!(first, second);
    }

    #[test]
    fn unions_with_different_members_stay_separate() {
        let mut arena = TypeArena::new();
        let a = arena.alloc(Type::StringLiteral("a".to_string()));
        let b = arena.alloc(Type::StringLiteral("b".to_string()));
        let c = arena.alloc(Type::StringLiteral("c".to_string()));

        assert_ne!(arena.alloc_union(vec![a, b]), arena.alloc_union(vec![a, c]));
    }

    // An alias names a union by wrapping it. The union is the id every identical union
    // shares, so it must stay unnamed: only uses that go through the alias print `Status`.
    #[test]
    fn an_alias_wraps_a_shared_union_and_leaves_it_unnamed() {
        let mut arena = TypeArena::new();
        let a = arena.alloc(Type::StringLiteral("a".to_string()));
        let b = arena.alloc(Type::StringLiteral("b".to_string()));
        let shared = arena.alloc_union(vec![a, b]);

        let status = arena.alloc_name("Status");
        let named = arena.alloc_named(status, shared);

        assert_ne!(named, shared);
        assert_eq!(arena.display_name(named), Some("Status"));
        assert_eq!(arena.display_name(shared), None);
        // The wrapper reads as the union it names, so every relation sees one type.
        assert_eq!(arena.get(named), arena.get(shared));
        // A later identical build still finds the shared one, not the wrapper.
        assert_eq!(arena.alloc_union(vec![a, b]), shared);
    }

    // The property the instantiation memo depends on: wrapping an application in an
    // alias leaves the application as it was, because the alias is a node of its own.
    #[test]
    fn an_alias_of_an_application_leaves_the_application_unchanged() {
        let mut arena = TypeArena::new();
        let number = arena.number();
        let body = arena.alloc(Type::Object(ObjectType::new(vec![PropertyEntry {
            name: "value".into(),
            type_id: number,
            optional: false,
            is_method: false,
        }])));
        let declaration = arena.alloc_name("Box");
        let application = arena.alloc_app(declaration, vec![number], body);

        let alias = arena.alloc_name("Alias");
        let named = arena.alloc_named(alias, application);

        assert_ne!(named, application);
        assert_eq!(arena.display_name(application), None);
        assert_eq!(arena.app_parts(application), Some(("Box", &[number][..])));
        assert_eq!(arena.display_name(named), Some("Alias"));
        assert!(arena.structurally_equal(application, named));
    }

    #[test]
    fn an_application_is_told_apart_by_its_declaration_and_arguments() {
        let mut arena = TypeArena::new();
        let (number, string) = (arena.number(), arena.string());
        let body = arena.alloc(Type::Object(ObjectType::new(Vec::new())));
        let declaration = arena.alloc_name("Box");
        let other = arena.alloc_name("Box");

        let of_number = arena.alloc_app(declaration, vec![number], body);
        // The same declaration, arguments and body are one id, so a memo can hand it out.
        assert_eq!(arena.alloc_app(declaration, vec![number], body), of_number);
        assert_ne!(arena.alloc_app(declaration, vec![string], body), of_number);
        // Two declarations that happen to share a name are still two declarations.
        assert_ne!(arena.alloc_app(other, vec![number], body), of_number);
    }

    // A built-in generic has one slot however many times it is asked for, which is what
    // makes every Promise<number> the same id, and Record's value type readable.
    #[test]
    fn a_built_in_generic_has_one_slot_and_record_gives_back_its_value_type() {
        let mut arena = TypeArena::new();
        let (number, string) = (arena.number(), arena.string());
        let body = arena.alloc(Type::Object(ObjectType::new(Vec::new())));

        let record = arena.builtin_slot("Record");
        assert_eq!(arena.builtin_slot("Record"), record);
        assert_ne!(arena.builtin_slot("Promise"), record);

        let counts = arena.alloc_app(record, vec![string, number], body);
        assert_eq!(arena.record_value_type(counts), Some(number));
        // Not a Record: no value type to give back.
        assert_eq!(arena.record_value_type(body), None);
        // A user's own generic named Record does not borrow the built-in's meaning.
        let users = arena.alloc_name("Record");
        let theirs = arena.alloc_app(users, vec![string, number], body);
        assert_eq!(arena.record_value_type(theirs), None);
    }

    // The digest is only a hint. If two different types ever share one, alloc() must
    // not return the other type's id: that would be a silent type confusion. It must
    // not hand out a fresh id on every call either, because literals are compared by
    // TypeId. A real 64-bit collision cannot be produced on demand with arbitrary
    // content, so the table is seeded with a wrong entry, which is exactly the state a
    // collision leaves behind.
    #[test]
    fn a_digest_match_on_different_content_still_gets_one_shared_id() {
        let mut arena = TypeArena::new();
        let (number, string) = (arena.number(), arena.string());
        let wanted = Type::Array(number);
        let unrelated = arena.alloc(Type::Array(string));
        arena.interned.insert(content_hash(&wanted), unrelated);

        let got = arena.alloc(wanted.clone());
        let again = arena.alloc(wanted.clone());

        assert_ne!(got, unrelated);
        assert!(*arena.get(got) == wanted);
        assert_eq!(got, again, "a collided type must still be interned once");
        assert!(arena.is_interned(got));
        assert!(arena.is_interned(unrelated));
    }

    // Two literals that really do collide under FxHasher, in the order a program meets
    // them: "variant19" owns the digest, "variant92" is the later one. Each must get one
    // id however often it is allocated, or `kind === "variant92"` finds no member.
    #[test]
    fn string_literals_that_share_a_digest_are_each_interned_once() {
        let mut arena = TypeArena::new();
        let first = arena.alloc(Type::StringLiteral("variant19".to_string()));
        let second = arena.alloc(Type::StringLiteral("variant92".to_string()));
        for _ in 0..3 {
            assert_eq!(
                arena.alloc(Type::StringLiteral("variant19".to_string())),
                first
            );
            assert_eq!(
                arena.alloc(Type::StringLiteral("variant92".to_string())),
                second
            );
        }
        assert_ne!(first, second);
    }

    // The general form: no matter how many of a family of similar literals are
    // allocated, every one of them keeps a single id.
    #[test]
    fn a_hundred_similar_literals_are_each_interned_once() {
        let mut arena = TypeArena::new();
        let ids: Vec<TypeId> = (0..100)
            .map(|i| arena.alloc(Type::StringLiteral(format!("variant{i}"))))
            .collect();
        for (i, id) in ids.iter().enumerate() {
            assert_eq!(
                arena.alloc(Type::StringLiteral(format!("variant{i}"))),
                *id,
                "variant{i} was given a second id"
            );
        }
    }

    #[test]
    fn a_digest_match_on_different_union_members_is_not_reused() {
        let mut arena = TypeArena::new();
        let a = arena.alloc(Type::StringLiteral("a".to_string()));
        let b = arena.alloc(Type::StringLiteral("b".to_string()));
        let c = arena.alloc(Type::StringLiteral("c".to_string()));
        let unrelated = arena.alloc_union(vec![a, b]);
        // What alloc_union will build for [a, c], seeded to point at the wrong slot.
        let members = vec![c, a];
        arena
            .interned_unions
            .insert(content_hash(&members), unrelated);

        let got = arena.alloc_union(vec![a, c]);

        let again = arena.alloc_union(vec![a, c]);

        assert_ne!(got, unrelated);
        assert!(arena.structurally_equal(got, again));
    }

    #[test]
    fn a_repeated_equality_question_is_answered_from_the_memo_in_either_order() {
        let mut arena = TypeArena::new();
        let number = arena.number();
        let one = object(&mut arena, vec![property("x", number, false)]);
        let two = object(&mut arena, vec![property("x", number, false)]);

        assert!(arena.structurally_equal_cached(one, two));
        assert!(arena.structurally_equal_cached(two, one));

        assert_eq!(
            arena.equality_cache.len(),
            1,
            "(a, b) and (b, a) share one entry"
        );
    }

    #[test]
    fn resolving_a_declaration_drops_equality_answers_given_while_it_was_empty() {
        let mut arena = TypeArena::new();
        let number = arena.number();
        let a = arena.alloc_ref();
        let b = arena.alloc_ref();
        assert!(arena.structurally_equal_cached(a, b));

        let body = object(&mut arena, vec![property("x", number, false)]);
        arena.resolve_ref(a, body);

        assert!(!arena.structurally_equal_cached(a, b));
    }

    #[test]
    fn union_keeps_composites_that_differ() {
        let mut arena = TypeArena::new();
        let (number, string) = (arena.number(), arena.string());
        let a = object(&mut arena, vec![property("a", number, false)]);
        let b = object(&mut arena, vec![property("a", string, false)]);

        let union = arena.alloc_union(vec![a, b]);

        assert!(matches!(arena.get(union), Type::Union(members) if members.len() == 2));
    }

    #[test]
    fn objects_that_differ_only_in_optionality_or_name_are_not_equal() {
        let mut arena = TypeArena::new();
        let number = arena.number();
        let required = object(&mut arena, vec![property("a", number, false)]);
        let optional = object(&mut arena, vec![property("a", number, true)]);
        let renamed = object(&mut arena, vec![property("b", number, false)]);

        assert!(!arena.structurally_equal(required, optional));
        assert!(!arena.structurally_equal(required, renamed));
    }

    #[test]
    fn object_property_order_at_construction_does_not_matter() {
        let mut arena = TypeArena::new();
        let (number, string) = (arena.number(), arena.string());
        let one = object(
            &mut arena,
            vec![property("z", string, false), property("a", number, false)],
        );
        let two = object(
            &mut arena,
            vec![property("a", number, false), property("z", string, false)],
        );

        assert!(arena.structurally_equal(one, two));
    }

    #[test]
    fn arrays_compare_by_element_shape() {
        let mut arena = TypeArena::new();
        let first_element = nested(&mut arena);
        let second_element = nested(&mut arena);
        let first = arena.alloc_fresh(Type::Array(first_element));
        let second = arena.alloc_fresh(Type::Array(second_element));
        let numbers = arena.alloc(Type::Array(arena.number()));

        assert!(arena.structurally_equal(first, second));
        assert!(!arena.structurally_equal(first, numbers));
    }

    #[test]
    fn functions_compare_by_parameters_and_return_type() {
        let mut arena = TypeArena::new();
        let (number, string) = (arena.number(), arena.string());
        let make = |arena: &mut TypeArena, param: TypeId, ret: TypeId, optional: bool| {
            arena.alloc_fresh(Type::Function(FunctionType {
                params: vec![Param {
                    type_id: param,
                    optional,
                    rest: false,
                    name: None,
                }],
                return_type: ret,
                type_params: Vec::new(),
                is_untyped: false,
            }))
        };
        let base = make(&mut arena, number, string, false);
        let same = make(&mut arena, number, string, false);
        let other_param = make(&mut arena, string, string, false);
        let other_return = make(&mut arena, number, number, false);
        let optional_param = make(&mut arena, number, string, true);

        assert!(arena.structurally_equal(base, same));
        assert!(!arena.structurally_equal(base, other_param));
        assert!(!arena.structurally_equal(base, other_return));
        assert!(!arena.structurally_equal(base, optional_param));
    }

    #[test]
    fn union_equality_ignores_member_order() {
        let mut arena = TypeArena::new();
        let (number, string) = (arena.number(), arena.string());
        let one = arena.alloc_union(vec![number, string]);
        let two = arena.alloc_union(vec![string, number]);

        assert!(arena.structurally_equal(one, two));
    }

    #[test]
    fn generic_parameters_are_equal_only_by_declaration_identity() {
        let mut arena = TypeArena::new();
        let same_declaration = TypeParameterId::new(10, 0);
        let other_declaration = TypeParameterId::new(50, 0);

        let first = arena.alloc_fresh(Type::GenericParameter(same_declaration, "T".into(), None));
        let again = arena.alloc_fresh(Type::GenericParameter(same_declaration, "T".into(), None));
        let lookalike =
            arena.alloc_fresh(Type::GenericParameter(other_declaration, "T".into(), None));

        assert!(arena.structurally_equal(first, again));
        assert!(
            !arena.structurally_equal(first, lookalike),
            "another `T` with the same name is a different type parameter"
        );
    }

    #[test]
    fn object_type_new_sorts_its_properties() {
        let arena = TypeArena::new();
        let number = arena.number();
        let built = ObjectType::new(vec![
            property("c", number, false),
            property("a", number, false),
            property("b", number, false),
        ]);

        let names: Vec<&str> = built.properties.iter().map(|p| &*p.name).collect();
        assert_eq!(names, ["a", "b", "c"]);
    }

    // ---- hash-consing (alloc) ----

    #[test]
    fn alloc_reuses_the_id_for_identical_anonymous_composites() {
        let mut arena = TypeArena::new();
        let (number, string) = (arena.number(), arena.string());

        let array_a = arena.alloc(Type::Array(number));
        let array_b = arena.alloc(Type::Array(number));
        assert_eq!(array_a, array_b);

        let object_a = shared_object(&mut arena, "a", number, false);
        let object_b = shared_object(&mut arena, "a", number, false);
        assert_eq!(object_a, object_b);

        let literal_a = arena.alloc(Type::StringLiteral("x".into()));
        let literal_b = arena.alloc(Type::StringLiteral("x".into()));
        assert_eq!(literal_a, literal_b);

        let function = |arena: &mut TypeArena| {
            arena.alloc(Type::Function(FunctionType {
                params: vec![Param::required(number)],
                return_type: string,
                type_params: Vec::new(),
                is_untyped: false,
            }))
        };
        assert_eq!(function(&mut arena), function(&mut arena));
    }

    #[test]
    fn alloc_keeps_different_content_apart() {
        let mut arena = TypeArena::new();
        let (number, string) = (arena.number(), arena.string());

        let numbers = arena.alloc(Type::Array(number));
        let strings = arena.alloc(Type::Array(string));
        assert_ne!(numbers, strings);

        let literal_a = arena.alloc(Type::StringLiteral("a".into()));
        let literal_b = arena.alloc(Type::StringLiteral("b".into()));
        assert_ne!(literal_a, literal_b);

        let yes = arena.alloc(Type::BooleanLiteral(true));
        let no = arena.alloc(Type::BooleanLiteral(false));
        assert_ne!(yes, no);

        let required = shared_object(&mut arena, "a", number, false);
        let optional = shared_object(&mut arena, "a", number, true);
        assert_ne!(required, optional);
    }

    #[test]
    fn object_property_order_does_not_stop_two_objects_sharing_an_id() {
        let mut arena = TypeArena::new();
        let (number, string) = (arena.number(), arena.string());
        let one = arena.alloc(Type::Object(ObjectType::new(vec![
            property("z", string, false),
            property("a", number, false),
        ])));
        let two = arena.alloc(Type::Object(ObjectType::new(vec![
            property("a", number, false),
            property("z", string, false),
        ])));

        assert_eq!(one, two, "ObjectType::new sorts, so the keys are identical");
    }

    #[test]
    fn number_literals_are_keyed_on_their_bits() {
        let mut arena = TypeArena::new();

        let positive_zero = arena.alloc(Type::NumberLiteral(0.0));
        let negative_zero = arena.alloc(Type::NumberLiteral(-0.0));
        assert_ne!(
            positive_zero, negative_zero,
            "0.0 and -0.0 have different bit patterns, so they are different keys"
        );
        assert!(
            !arena.structurally_equal(positive_zero, negative_zero),
            "structurally_equal must agree with the intern key about signed zero"
        );

        let nan_a = arena.alloc(Type::NumberLiteral(f64::NAN));
        let nan_b = arena.alloc(Type::NumberLiteral(f64::NAN));
        assert_eq!(
            nan_a, nan_b,
            "a NaN literal is reused, not allocated forever"
        );

        let one_a = arena.alloc(Type::NumberLiteral(1.0));
        let one_b = arena.alloc(Type::NumberLiteral(1.0));
        assert_eq!(one_a, one_b);
    }

    #[test]
    fn generic_parameters_collapse_only_within_one_declaration() {
        let mut arena = TypeArena::new();
        let declaration = TypeParameterId::new(10, 0);
        let other_declaration = TypeParameterId::new(50, 0);

        let first = arena.alloc(Type::GenericParameter(declaration, "T".into(), None));
        let again = arena.alloc(Type::GenericParameter(declaration, "T".into(), None));
        let lookalike = arena.alloc(Type::GenericParameter(other_declaration, "T".into(), None));

        assert_eq!(first, again);
        assert_ne!(
            first, lookalike,
            "another `T` with the same name stays distinct"
        );
    }

    #[test]
    fn parameter_names_keep_otherwise_identical_functions_apart() {
        let mut arena = TypeArena::new();
        let (number, string) = (arena.number(), arena.string());
        let named = |arena: &mut TypeArena, name: &str| {
            arena.alloc(Type::Function(FunctionType {
                params: vec![Param {
                    type_id: number,
                    optional: false,
                    rest: false,
                    name: Some(name.into()),
                }],
                return_type: string,
                type_params: Vec::new(),
                is_untyped: false,
            }))
        };

        let x = named(&mut arena, "x");
        let y = named(&mut arena, "y");

        // A "missing argument" diagnostic names the parameter, so merging these
        // would let one function report the other's parameter name.
        assert_ne!(x, y);
        assert!(
            arena.structurally_equal(x, y),
            "still the same type by shape; only the ids differ"
        );
    }

    // Pins the limit of union interning: the key is order-sensitive, so a reordered
    // union is a separate id (see interned_unions for why the key is not sorted).
    // structurally_equal still treats the two as one type.
    #[test]
    fn reordered_unions_are_equal_by_shape_but_keep_separate_ids() {
        let mut arena = TypeArena::new();
        let (number, string) = (arena.number(), arena.string());

        let one = arena.alloc_union(vec![number, string]);
        let two = arena.alloc_union(vec![string, number]);

        assert_ne!(
            one, two,
            "order-sensitive key: reordering is a different id"
        );
        assert!(arena.structurally_equal(one, two));
    }

    #[test]
    fn clear_returns_to_the_primitive_baseline_and_keeps_capacity() {
        use crate::types::PropertyEntry;

        let mut arena = TypeArena::new();
        let number = arena.number();
        let object = arena.alloc_fresh(Type::Object(ObjectType::new(vec![PropertyEntry {
            name: "value".into(),
            type_id: number,
            optional: false,
            is_method: false,
        }])));
        let wide = arena.alloc_name("Wide");
        let named = arena.alloc_named(wide, object);
        let capacity_before = arena.stats().capacity;
        assert!(arena.len() > FIXED_SLOTS as usize);

        arena.clear();

        assert_eq!(arena.len(), FIXED_SLOTS as usize);
        assert!(arena.stats().capacity >= capacity_before);
        assert_eq!(arena.number(), TypeId(0));
        assert_eq!(arena.void(), TypeId(9));
        assert_eq!(arena.stats().named_types, 0);
        assert_eq!(arena.stats().interned_types, 0);
        assert_eq!(arena.display_name(named), None);

        let fresh = arena.alloc(Type::Array(arena.number()));
        assert_eq!(
            fresh,
            TypeId(FIXED_SLOTS),
            "ids restart right after the primitives"
        );
    }

    #[test]
    fn generation_moves_only_when_an_existing_id_changes_meaning() {
        let mut arena = TypeArena::new();
        let start = arena.generation();

        arena.alloc(Type::Array(arena.number()));
        assert_eq!(arena.generation(), start, "a new id has no stale answers");

        let declaration = arena.alloc_ref();
        assert_eq!(arena.generation(), start);

        let body = arena.alloc(Type::Object(ObjectType::new(Vec::new())));
        arena.resolve_ref(declaration, body);
        assert_ne!(
            arena.generation(),
            start,
            "resolving a declaration changes what its id reads as"
        );

        let after_resolve = arena.generation();
        arena.clear();
        assert_ne!(
            arena.generation(),
            after_resolve,
            "clear() starts a new generation"
        );
    }

    #[test]
    fn stats_count_interned_and_named_types() {
        let mut arena = TypeArena::new();
        let before = arena.stats();
        let array = arena.alloc(Type::Array(arena.number()));
        let again = arena.alloc(Type::Array(arena.number()));
        assert_eq!(array, again);

        let after = arena.stats();
        assert_eq!(after.type_count, before.type_count + 1);
        assert_eq!(after.interned_types, before.interned_types + 1);
    }

    #[test]
    fn deep_recursive_comparison_spills_past_the_inline_stack() {
        use crate::types::PropertyEntry;

        // A chain deeper than PairStack's sixteen inline slots, so the recursion
        // guard has to move to the heap and still give the right answer.
        let mut arena = TypeArena::new();
        let mut left = arena.number();
        let mut right = arena.number();
        for _ in 0..40 {
            left = arena.alloc(Type::Object(ObjectType::new(vec![PropertyEntry {
                name: "next".into(),
                type_id: left,
                optional: false,
                is_method: false,
            }])));
            right = arena.alloc(Type::Object(ObjectType::new(vec![PropertyEntry {
                name: "next".into(),
                type_id: right,
                optional: false,
                is_method: false,
            }])));
        }
        assert!(arena.structurally_equal(left, right));
    }

    #[test]
    fn primitives_keep_their_fixed_slots() {
        let arena = TypeArena::new();
        assert_eq!(arena.number(), TypeId(0));
        assert_eq!(arena.void(), TypeId(9));
    }

    // ---- carve-outs: types that must keep an id of their own ----

    #[test]
    fn declarations_are_never_shared_with_each_other_or_with_an_empty_object() {
        let mut arena = TypeArena::new();

        let first = arena.alloc_ref();
        let second = arena.alloc_ref();
        let empty = arena.alloc(Type::Object(ObjectType::new(Vec::new())));

        assert_ne!(first, second);
        assert_ne!(first, empty);
        assert_ne!(second, empty);
    }

    #[test]
    fn a_resolved_declaration_never_joins_the_intern_table() {
        let mut arena = TypeArena::new();
        let number = arena.number();
        let declaration = arena.alloc_ref();
        let body = shared_object(&mut arena, "a", number, false);
        arena.resolve_ref(declaration, body);

        let same_shape = shared_object(&mut arena, "a", number, false);

        assert_ne!(
            declaration, same_shape,
            "other types already hold the declaration's id, so it is never merged with a shape"
        );
        assert_eq!(
            same_shape, body,
            "the body is an ordinary interned shape and is shared like any other"
        );
        assert!(arena.structurally_equal(declaration, same_shape));
    }

    #[test]
    fn a_declaration_reads_as_empty_until_resolved_and_as_its_body_after() {
        let mut arena = TypeArena::new();
        let number = arena.number();
        let declaration = arena.alloc_ref();

        assert!(matches!(arena.get(declaration), Type::Object(o) if o.properties.is_empty()));

        let body = object(&mut arena, vec![property("a", number, false)]);
        arena.resolve_ref(declaration, body);

        assert!(matches!(arena.get(declaration), Type::Object(o) if o.properties.len() == 1));
    }

    #[test]
    fn resolving_a_declaration_does_not_change_its_id_or_add_a_slot() {
        let mut arena = TypeArena::new();
        let declaration = arena.alloc_ref();
        let list = arena.alloc(Type::Array(declaration));
        let body = arena.alloc(Type::Object(ObjectType::new(Vec::new())));
        let slots = arena.len();

        arena.resolve_ref(declaration, body);

        assert_eq!(arena.len(), slots);
        assert!(
            matches!(arena.get(list), Type::Array(element) if *element == declaration),
            "a type built on the id before it was resolved still holds that id"
        );
    }

    #[test]
    fn two_declarations_sharing_a_body_keep_their_own_names() {
        let mut arena = TypeArena::new();
        let number = arena.number();
        let body = object(&mut arena, vec![property("id", number, false)]);
        let dog = arena.alloc_ref();
        let cat = arena.alloc_ref();
        arena.resolve_ref(dog, body);
        arena.resolve_ref(cat, body);
        arena.name_ref(dog, "Dog");
        arena.name_ref(cat, "Cat");

        assert_eq!(arena.display_name(dog), Some("Dog"));
        assert_eq!(arena.display_name(cat), Some("Cat"));
        assert_eq!(
            arena.display_name(body),
            None,
            "naming a declaration never names its body"
        );
        assert!(arena.structurally_equal(dog, cat));
    }

    #[test]
    fn a_failed_declaration_keeps_reading_as_an_empty_object() {
        let mut arena = TypeArena::new();
        let declaration = arena.alloc_ref();
        let generation = arena.generation();

        arena.fail_ref(declaration);

        assert!(matches!(arena.get(declaration), Type::Object(o) if o.properties.is_empty()));
        assert_eq!(
            arena.generation(),
            generation,
            "no answer ever rested on a body"
        );
    }

    #[test]
    fn declaration_names_are_counted_and_cleared_with_the_arena() {
        let mut arena = TypeArena::new();
        let declaration = arena.alloc_ref();
        arena.name_ref(declaration, "Node");
        assert_eq!(arena.stats().named_types, 1);

        arena.clear();
        assert_eq!(arena.stats().named_types, 0);
    }

    #[test]
    fn two_open_declarations_never_merge_through_a_shape_that_mentions_them() {
        let mut arena = TypeArena::new();
        let first = arena.alloc_ref();
        let second = arena.alloc_ref();

        // Both are still the same empty `{}` here. The key hashes the ids, not
        // what they point at, so these must stay two different arrays.
        let first_list = arena.alloc(Type::Array(first));
        let second_list = arena.alloc(Type::Array(second));

        assert_ne!(first_list, second_list);
    }

    #[test]
    fn alloc_fresh_never_reuses_an_id_even_for_identical_content() {
        let mut arena = TypeArena::new();
        let number = arena.number();

        let shared = arena.alloc(Type::Array(number));
        let fresh_a = arena.alloc_fresh(Type::Array(number));
        let fresh_b = arena.alloc_fresh(Type::Array(number));
        let after = arena.alloc(Type::Array(number));

        assert_ne!(fresh_a, shared);
        assert_ne!(fresh_a, fresh_b);
        assert_eq!(
            after, shared,
            "alloc_fresh does not disturb the intern table"
        );
    }

    #[test]
    fn naming_a_shared_array_through_an_alias_does_not_rename_the_shared_type() {
        let mut arena = TypeArena::new();
        let number = arena.number();

        let shared = arena.alloc(Type::Array(number));
        let slot = arena.alloc_name("Scores");
        let scores = arena.alloc_named(slot, shared);

        assert_eq!(arena.display_name(scores), Some("Scores"));
        assert_eq!(arena.display_name(shared), None);
        assert_eq!(arena.alloc(Type::Array(number)), shared);
        assert!(arena.structurally_equal(scores, shared));
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "alloc_ref")]
    fn resolving_an_id_that_is_not_a_declaration_is_caught_in_debug_builds() {
        let mut arena = TypeArena::new();
        let number = arena.number();
        let shared = arena.alloc(Type::Array(number));
        arena.resolve_ref(shared, number);
    }

    // `interface N { next: N | null }` built the way namespace::resolve builds
    // it: the Ref first, members resolved against it, then resolve_ref(). Afterwards
    // the body refers to the declaration's own id, so the arena is no longer acyclic.
    fn recursive_node(arena: &mut TypeArena, tail: TypeId) -> TypeId {
        let declaration = arena.alloc_ref();
        let next = arena.alloc_union(vec![declaration, tail]);
        let body = object(arena, vec![property("next", next, false)]);
        arena.resolve_ref(declaration, body);
        declaration
    }

    // Comparing two identically shaped recursive types goes a -> b -> a -> b ... and
    // only terminates because a pair already on the stack is assumed equal
    // (coinduction). The answer must be equal, and the call must return.
    #[test]
    fn structurally_equal_terminates_on_identical_recursive_types() {
        let mut arena = TypeArena::new();
        let null = arena.null();
        let a = recursive_node(&mut arena, null);
        let b = recursive_node(&mut arena, null);

        assert_ne!(a, b);
        assert!(arena.structurally_equal(a, b));
    }

    // Same, but the shapes really differ (null vs undefined tail). Must
    // terminate AND say not equal: assuming "equal" for a re-entered pair must
    // not hide a difference elsewhere in the type.
    #[test]
    fn structurally_equal_terminates_on_different_recursive_types() {
        let mut arena = TypeArena::new();
        let (null, undefined) = (arena.null(), arena.undefined());
        let a = recursive_node(&mut arena, null);
        let c = recursive_node(&mut arena, undefined);

        assert!(!arena.structurally_equal(a, c));
    }

    // The realistic way to reach it: `type Either = A | B` goes through
    // alloc_union, whose dedup calls structurally_equal on the members.
    #[test]
    fn alloc_union_of_identical_recursive_types_terminates() {
        let mut arena = TypeArena::new();
        let null = arena.null();
        let a = recursive_node(&mut arena, null);
        let b = recursive_node(&mut arena, null);

        let union = arena.alloc_union(vec![a, b]);

        assert!(matches!(arena.get(union), Type::Object(_) | Type::Union(_)));
    }

    // A method and a function-valued property must stay distinct types. The intern
    // key separates them (PropertyEntry hashes is_method) and subtyping treats them
    // differently (method parameters are bivariant), so structurally_equal has to
    // compare is_method too: otherwise alloc_union would merge the two into one
    // union member, while tsc keeps both.
    #[test]
    fn a_method_and_a_function_valued_property_are_not_the_same_shape() {
        let mut arena = TypeArena::new();
        let void = arena.void();
        let function = arena.alloc(Type::Function(FunctionType {
            params: vec![],
            return_type: void,
            type_params: Vec::new(),
            is_untyped: false,
        }));
        let entry = |is_method: bool| PropertyEntry {
            name: "f".into(),
            type_id: function,
            optional: false,
            is_method,
        };

        // The intern key agrees they differ (always held) ...
        let interned_method = arena.alloc(Type::Object(ObjectType::new(vec![entry(true)])));
        let interned_field = arena.alloc(Type::Object(ObjectType::new(vec![entry(false)])));
        assert_ne!(interned_method, interned_field);

        // ... and structural equality must agree with it: is_method is part of the shape.
        let method = object(&mut arena, vec![entry(true)]);
        let field = object(&mut arena, vec![entry(false)]);
        assert!(!arena.structurally_equal(method, field));
    }

    // `type Age = number` must not wrap slot 0: tsc prints such an alias as the primitive,
    // and every `number` shares that one id. alloc_named hands a fixed slot back as it is.
    #[test]
    fn a_fixed_slot_is_never_wrapped_in_a_name() {
        let mut arena = TypeArena::new();
        let fixed = [
            arena.number(),
            arena.string(),
            arena.boolean(),
            arena.null(),
            arena.undefined(),
            arena.any(),
            arena.unknown(),
            arena.error(),
            arena.never(),
            arena.void(),
        ];

        for id in fixed {
            let slot = arena.alloc_name("Renamed");
            assert_eq!(
                arena.alloc_named(slot, id),
                id,
                "fixed slot {id:?} was wrapped"
            );
            assert_eq!(arena.display_name(id), None);
        }
    }
}

// The construction rules and the lazy reduction of intersections, one test per rule in
// LLD 1.13. They go through alloc_intersection, which is the only way to build the node.
#[cfg(test)]
mod intersection_tests {
    use super::*;
    use crate::types::{ObjectType, PropertyEntry, TypeParameterId};

    fn property(name: &str, type_id: TypeId) -> PropertyEntry {
        PropertyEntry {
            name: name.into(),
            type_id,
            optional: false,
            is_method: false,
        }
    }

    // `{ <name>: <type_id> }`, shared through alloc() like every anonymous shape.
    fn shape(arena: &mut TypeArena, name: &str, type_id: TypeId) -> TypeId {
        arena.alloc(Type::Object(ObjectType::new(vec![property(name, type_id)])))
    }

    fn literal(arena: &mut TypeArena, text: &str) -> TypeId {
        arena.alloc(Type::StringLiteral(text.to_string()))
    }

    fn parameter(arena: &mut TypeArena, name: &str, at: u32) -> TypeId {
        arena.alloc(Type::GenericParameter(
            TypeParameterId::new(at, 0),
            name.into(),
            None,
        ))
    }

    fn members(arena: &TypeArena, id: TypeId) -> Vec<TypeId> {
        match arena.get(id) {
            Type::Intersection(members) => members.clone(),
            other => panic!("expected an intersection, found {other:?}"),
        }
    }

    fn build(arena: &mut TypeArena, parts: &[TypeId]) -> TypeId {
        arena
            .alloc_intersection(parts.to_vec())
            .expect("this intersection is small enough to build")
    }

    // Rule 1: the written order is the identity.
    #[test]
    fn two_orders_are_two_ids_and_one_order_is_one_id() {
        let mut arena = TypeArena::new();
        let number = arena.number();
        let string = arena.string();
        let a = shape(&mut arena, "a", number);
        let b = shape(&mut arena, "b", string);

        let ab = build(&mut arena, &[a, b]);
        let ba = build(&mut arena, &[b, a]);
        assert_ne!(ab, ba);
        assert_eq!(
            build(&mut arena, &[a, b]),
            ab,
            "the same list is the same id"
        );
        assert_eq!(members(&arena, ab), [a, b]);
        assert_eq!(members(&arena, ba), [b, a]);
    }

    // The relation view of the same two ids: equal, though not identical.
    #[test]
    fn as_a_relation_the_two_orders_are_one_type() {
        let mut arena = TypeArena::new();
        let (number, string) = (arena.number(), arena.string());
        let a = shape(&mut arena, "a", number);
        let b = shape(&mut arena, "b", string);
        let ab = build(&mut arena, &[a, b]);
        let ba = build(&mut arena, &[b, a]);
        assert!(arena.structurally_equal(ab, ba));
    }

    #[test]
    fn a_nested_intersection_is_spread_into_its_members() {
        let mut arena = TypeArena::new();
        let (number, string, boolean) = (arena.number(), arena.string(), arena.boolean());
        let a = shape(&mut arena, "a", number);
        let b = shape(&mut arena, "b", string);
        let c = shape(&mut arena, "c", boolean);

        let ab = build(&mut arena, &[a, b]);
        let nested = build(&mut arena, &[ab, c]);
        assert_eq!(nested, build(&mut arena, &[a, b, c]));
        assert_eq!(members(&arena, nested), [a, b, c]);
    }

    // `type AB = A & B; AB & C` is `A & B & C`: the alias is a wrapper, and a wrapper's
    // contents are known when it exists.
    #[test]
    fn an_alias_of_an_intersection_is_flattened_like_the_intersection_itself() {
        let mut arena = TypeArena::new();
        let (number, string, boolean) = (arena.number(), arena.string(), arena.boolean());
        let a = shape(&mut arena, "a", number);
        let b = shape(&mut arena, "b", string);
        let c = shape(&mut arena, "c", boolean);

        let ab = build(&mut arena, &[a, b]);
        let slot = arena.alloc_name("AB");
        let alias = arena.alloc_named(slot, ab);
        assert_eq!(
            build(&mut arena, &[alias, c]),
            build(&mut arena, &[a, b, c])
        );
    }

    #[test]
    fn unknown_adds_nothing_and_only_unknown_is_unknown() {
        let mut arena = TypeArena::new();
        let (number, unknown) = (arena.number(), arena.unknown());
        let a = shape(&mut arena, "a", number);
        assert_eq!(build(&mut arena, &[a, unknown]), a);
        assert_eq!(build(&mut arena, &[unknown, unknown]), unknown);
        assert_eq!(build(&mut arena, &[]), unknown);
    }

    #[test]
    fn never_wins_even_over_any_and_any_wins_over_the_rest() {
        let mut arena = TypeArena::new();
        let (number, any, never) = (arena.number(), arena.any(), arena.never());
        let a = shape(&mut arena, "a", number);
        assert_eq!(build(&mut arena, &[a, never]), never);
        assert_eq!(build(&mut arena, &[any, never]), never);
        assert_eq!(build(&mut arena, &[never, any]), never);
        assert_eq!(build(&mut arena, &[any, a]), any);
    }

    // An error that was already reported stays the sentinel instead of turning into `any`.
    #[test]
    fn the_error_type_is_kept_over_any() {
        let mut arena = TypeArena::new();
        let (any, error) = (arena.any(), arena.error());
        assert_eq!(build(&mut arena, &[any, error]), error);
    }

    // Rule 5: a literal is more specific than its own primitive.
    #[test]
    fn a_literal_wins_over_its_primitive() {
        let mut arena = TypeArena::new();
        let (string, number, boolean) = (arena.string(), arena.number(), arena.boolean());
        let a = literal(&mut arena, "a");
        let one = arena.alloc(Type::NumberLiteral(1.0));
        let truth = arena.alloc(Type::BooleanLiteral(true));

        assert_eq!(build(&mut arena, &[a, string]), a);
        assert_eq!(build(&mut arena, &[string, a]), a);
        assert_eq!(build(&mut arena, &[one, number]), one);
        assert_eq!(build(&mut arena, &[truth, boolean]), truth);
    }

    #[test]
    fn primitives_and_literals_that_cannot_meet_are_never() {
        let mut arena = TypeArena::new();
        let (string, number, never) = (arena.string(), arena.number(), arena.never());
        let (a, b) = (literal(&mut arena, "a"), literal(&mut arena, "b"));
        let one = arena.alloc(Type::NumberLiteral(1.0));
        let branded = shape(&mut arena, "z", number);

        assert_eq!(build(&mut arena, &[string, number]), never);
        assert_eq!(build(&mut arena, &[a, b]), never);
        assert_eq!(build(&mut arena, &[a, number]), never);
        assert_eq!(build(&mut arena, &[one, string]), never);
        // Anywhere in the list: an object does not rescue two primitives that cannot meet.
        assert_eq!(build(&mut arena, &[number, string, branded]), never);
    }

    // `string & { __brand: "id" }` is how a branded primitive is written, so an object
    // never cancels a primitive.
    #[test]
    fn a_branded_primitive_stays_an_intersection() {
        let mut arena = TypeArena::new();
        let string = arena.string();
        let id = literal(&mut arena, "id");
        let brand = shape(&mut arena, "__brand", id);
        let branded = build(&mut arena, &[string, brand]);
        assert_eq!(members(&arena, branded), [string, brand]);

        let array = arena.alloc(Type::Array(string));
        let built1 = build(&mut arena, &[string, array]);
        assert_eq!(members(&arena, built1), [string, array]);
    }

    #[test]
    fn null_and_undefined_share_a_value_with_nothing_but_themselves() {
        let mut arena = TypeArena::new();
        let (null, undefined, void, never) =
            (arena.null(), arena.undefined(), arena.void(), arena.never());
        let (number, string) = (arena.number(), arena.string());
        let object = shape(&mut arena, "a", number);
        let t = parameter(&mut arena, "T", 1);

        assert_eq!(build(&mut arena, &[null, object]), never);
        assert_eq!(build(&mut arena, &[undefined, object]), never);
        assert_eq!(build(&mut arena, &[null, undefined]), never);
        assert_eq!(build(&mut arena, &[null, string]), never);
        // `undefined & void` is `undefined`, the more specific of the two.
        assert_eq!(build(&mut arena, &[void, undefined]), undefined);
        assert_eq!(build(&mut arena, &[undefined, void]), undefined);
        // A type parameter could be null, so it is kept.
        let built2 = build(&mut arena, &[null, t]);
        assert_eq!(members(&arena, built2), [null, t]);
    }

    // tsc: `void & A` stays, `void & string` is never.
    #[test]
    fn void_is_kept_next_to_an_object_and_cancels_against_another_primitive() {
        let mut arena = TypeArena::new();
        let (void, string, number, never) =
            (arena.void(), arena.string(), arena.number(), arena.never());
        let object = shape(&mut arena, "a", number);
        let built3 = build(&mut arena, &[void, object]);
        assert_eq!(members(&arena, built3), [void, object]);
        assert_eq!(build(&mut arena, &[void, string]), never);
    }

    // Rule 7.
    #[test]
    fn an_empty_object_is_dropped_next_to_an_object_but_not_next_to_a_primitive() {
        let mut arena = TypeArena::new();
        let (number, string) = (arena.number(), arena.string());
        let empty = arena.alloc(Type::Object(ObjectType::new(Vec::new())));
        let object = shape(&mut arena, "a", number);
        let function = arena.alloc(Type::Function(crate::types::FunctionType {
            params: Vec::new(),
            return_type: number,
            type_params: Vec::new(),
            is_untyped: false,
        }));
        let t = parameter(&mut arena, "T", 1);

        assert_eq!(build(&mut arena, &[object, empty]), object);
        assert_eq!(build(&mut arena, &[empty, function]), function);
        assert_eq!(build(&mut arena, &[empty, empty]), empty);
        // `string & {}` and `T & {}` mean "not null", so the `{}` stays.
        let built4 = build(&mut arena, &[string, empty]);
        assert_eq!(members(&arena, built4), [string, empty]);
        let built5 = build(&mut arena, &[t, empty]);
        assert_eq!(members(&arena, built5), [t, empty]);
    }

    // A Promise<number> is an App over an empty body. It is not the anonymous `{}`.
    #[test]
    fn an_application_over_an_empty_body_is_not_mistaken_for_the_empty_object() {
        let mut arena = TypeArena::new();
        let (number, string) = (arena.number(), arena.string());
        let body = arena.alloc(Type::Object(ObjectType::new(Vec::new())));
        let slot = arena.builtin_slot("Promise");
        let promise = arena.alloc_app(slot, vec![number], body);
        let object = shape(&mut arena, "a", string);

        let both = build(&mut arena, &[promise, object]);
        assert_eq!(members(&arena, both), [promise, object]);
    }

    #[test]
    fn duplicates_are_removed_by_id_and_the_first_one_stays_first() {
        let mut arena = TypeArena::new();
        let (number, string, boolean) = (arena.number(), arena.string(), arena.boolean());
        let a = shape(&mut arena, "a", number);
        let b = shape(&mut arena, "b", string);
        let c = shape(&mut arena, "c", boolean);

        let deduped = build(&mut arena, &[b, a, b, c, a]);
        assert_eq!(members(&arena, deduped), [b, a, c]);
        assert_eq!(
            build(&mut arena, &[a, a]),
            a,
            "one member left is that member"
        );
    }

    // Rule 6.
    #[test]
    fn a_union_is_distributed_and_member_order_inside_each_choice_is_the_written_one() {
        let mut arena = TypeArena::new();
        let (number, string, boolean) = (arena.number(), arena.string(), arena.boolean());
        let a = shape(&mut arena, "a", number);
        let b = shape(&mut arena, "b", string);
        let c = shape(&mut arena, "c", boolean);
        let a_or_b = arena.alloc_union(vec![a, b]);

        let distributed = build(&mut arena, &[a_or_b, c]);
        let ac = build(&mut arena, &[a, c]);
        let bc = build(&mut arena, &[b, c]);
        let expected = arena.alloc_union(vec![ac, bc]);
        assert_eq!(distributed, expected);

        let reversed = build(&mut arena, &[c, a_or_b]);
        let ca = build(&mut arena, &[c, a]);
        let cb = build(&mut arena, &[c, b]);
        let expected_reversed = arena.alloc_union(vec![ca, cb]);
        assert_eq!(reversed, expected_reversed);
        assert_ne!(distributed, reversed);
    }

    // Each choice is built by the same function, so what is left to reduce is reduced.
    #[test]
    fn distribution_reduces_each_choice() {
        let mut arena = TypeArena::new();
        let string = arena.string();
        let (a, b, c) = (
            literal(&mut arena, "a"),
            literal(&mut arena, "b"),
            literal(&mut arena, "c"),
        );
        let abc = arena.alloc_union(vec![a, b, c]);
        assert_eq!(build(&mut arena, &[abc, string]), abc);

        let ab = arena.alloc_union(vec![a, b]);
        let bc = arena.alloc_union(vec![b, c]);
        assert_eq!(build(&mut arena, &[ab, bc]), b);
    }

    #[test]
    fn a_distribution_that_is_too_large_is_refused() {
        let mut arena = TypeArena::new();
        // Six unions of ten distinct literals each: 10^6 combinations.
        let unions: Vec<TypeId> = (0..6)
            .map(|set| {
                let literals: Vec<TypeId> = (0..10)
                    .map(|index| literal(&mut arena, &format!("{set}_{index}")))
                    .collect();
                arena.alloc_union(literals)
            })
            .collect();
        assert_eq!(arena.alloc_intersection(unions), Err(TooComplex));
    }

    #[test]
    fn a_type_parameter_is_an_opaque_member() {
        let mut arena = TypeArena::new();
        let (unknown, never) = (arena.unknown(), arena.never());
        let t = parameter(&mut arena, "T", 1);
        let u = parameter(&mut arena, "U", 2);

        let built6 = build(&mut arena, &[t, u]);
        assert_eq!(members(&arena, built6), [t, u]);
        assert_eq!(build(&mut arena, &[t, unknown]), t);
        assert_eq!(build(&mut arena, &[t, t]), t);
        assert_eq!(build(&mut arena, &[t, never]), never);
    }

    // The point of rule 2: construction decides from ids and wrappers, so an answer made
    // while a declaration was unresolved is the answer after it is resolved.
    #[test]
    fn construction_gives_the_same_answer_before_and_after_a_declaration_is_resolved() {
        let mut arena = TypeArena::new();
        let (number, string) = (arena.number(), arena.string());
        let empty = arena.alloc(Type::Object(ObjectType::new(Vec::new())));
        let declaration = arena.alloc_ref();

        let with_primitive = build(&mut arena, &[string, declaration]);
        let with_empty = build(&mut arena, &[empty, declaration]);

        let body = shape(&mut arena, "a", number);
        arena.resolve_ref(declaration, body);

        assert_eq!(build(&mut arena, &[string, declaration]), with_primitive);
        assert_eq!(build(&mut arena, &[empty, declaration]), with_empty);
        assert_eq!(
            with_empty, declaration,
            "`{{}} & Declaration` is the declaration"
        );
        assert_eq!(members(&arena, with_primitive), [string, declaration]);
    }

    // Reduction. tsc keeps `{ kind: "a" } & { kind: "b" }` as an intersection and reduces
    // it when something asks.
    #[test]
    fn a_discriminant_conflict_is_never_but_the_node_stays_an_intersection() {
        let mut arena = TypeArena::new();
        let (a, b) = (literal(&mut arena, "a"), literal(&mut arena, "b"));
        let kind_a = shape(&mut arena, "kind", a);
        let kind_b = shape(&mut arena, "kind", b);

        let both = build(&mut arena, &[kind_a, kind_b]);
        assert!(matches!(arena.get(both), Type::Intersection(_)));
        assert!(arena.intersection_reduces_to_never(both));
    }

    #[test]
    fn a_conflict_between_types_that_are_not_literals_is_not_a_discriminant() {
        let mut arena = TypeArena::new();
        let (number, string) = (arena.number(), arena.string());
        let x_number = shape(&mut arena, "x", number);
        let x_string = shape(&mut arena, "x", string);
        let both = build(&mut arena, &[x_number, x_string]);
        assert!(!arena.intersection_reduces_to_never(both));
    }

    #[test]
    fn members_that_agree_or_do_not_overlap_are_not_empty() {
        let mut arena = TypeArena::new();
        let (a, number) = (literal(&mut arena, "a"), arena.number());
        let kind_a = shape(&mut arena, "kind", a);
        let size = shape(&mut arena, "size", number);
        let both = build(&mut arena, &[kind_a, size]);
        assert!(!arena.intersection_reduces_to_never(both));
        assert!(
            !arena.intersection_reduces_to_never(kind_a),
            "not an intersection"
        );
    }

    // A union of literals is a discriminant too.
    #[test]
    fn a_property_of_overlapping_literal_unions_is_not_a_conflict_and_a_disjoint_one_is() {
        let mut arena = TypeArena::new();
        let (a, b, c) = (
            literal(&mut arena, "a"),
            literal(&mut arena, "b"),
            literal(&mut arena, "c"),
        );
        let a_or_b = arena.alloc_union(vec![a, b]);
        let b_or_c = arena.alloc_union(vec![b, c]);
        let only_c = shape(&mut arena, "kind", c);
        let left = shape(&mut arena, "kind", a_or_b);
        let right = shape(&mut arena, "kind", b_or_c);

        let overlapping = build(&mut arena, &[left, right]);
        assert!(!arena.intersection_reduces_to_never(overlapping));
        let disjoint = build(&mut arena, &[left, only_c]);
        assert!(arena.intersection_reduces_to_never(disjoint));
    }

    // The reduction reads bodies, so an answer given while a declaration was unresolved
    // must not survive its resolution (empty_intersections is emptied by resolve_ref).
    #[test]
    fn the_reduction_is_asked_again_after_a_declaration_is_resolved() {
        let mut arena = TypeArena::new();
        let (a, b) = (literal(&mut arena, "a"), literal(&mut arena, "b"));
        let kind_a = shape(&mut arena, "kind", a);
        let kind_b = shape(&mut arena, "kind", b);
        let declaration = arena.alloc_ref();

        let both = build(&mut arena, &[declaration, kind_a]);
        assert!(
            !arena.intersection_reduces_to_never(both),
            "unresolved reads as `{{}}`"
        );

        arena.resolve_ref(declaration, kind_b);
        assert!(arena.intersection_reduces_to_never(both));
    }
}
