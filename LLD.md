# tsox: Low-Level Design (Draft 1)

**Status:** design before code. Rust snippets are interface and data-layout sketches, not final code.
**Companion:** `tsox-HLD.md` has the decision records and the reasons for each choice. Section numbers below match the HLD's ADRs where noted.
**[VERIFY]** marks library behavior to confirm with a spike before relying on it.

Contents: 1 Arena and type nodes, 2 Memory, 3 Stable identity and portable types, 4 Module graph, 5 Concurrency, 6 Query layer, 7 Checker integration and migration, 8 Determinism and limits, 9 Tests and measurements, 10 Risks and open questions.

---

## 1. Arena and type nodes (ADR-1)

### 1.1 Handles and the primitive split

```rust
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct TypeId(u32);

pub const PRIM_COUNT: u32 = 10;
impl TypeId {
    pub const NUMBER:    Self = Self(0);
    pub const STRING:    Self = Self(1);
    pub const BOOLEAN:   Self = Self(2);
    pub const NULL:      Self = Self(3);
    pub const UNDEFINED: Self = Self(4);
    pub const ANY:       Self = Self(5);
    pub const UNKNOWN:   Self = Self(6);
    pub const ERROR:     Self = Self(7);
    pub const NEVER:     Self = Self(8);
    pub const VOID:      Self = Self(9);
    #[inline] pub fn is_primitive(self) -> bool { self.0 < PRIM_COUNT }
    #[inline] fn slot(self) -> usize { (self.0 - PRIM_COUNT) as usize }
}
```

- Primitive ids have no storage and are identical in every arena, so `TypeId::NUMBER` means the same thing everywhere (this matters for portable data).
- Non-primitive ids index `Arena::nodes`.
- A `TypeId` is **only meaningful with the arena that issued it**. Never put one in a type that outlives the arena or crosses threads. A newtype wrapper with a phantom arena lifetime can make this a compile-time error (see 1.11).
- Literals (string, number, boolean) carry payloads, so they are non-primitive nodes and are interned.

### 1.2 Node set

```rust
#[derive(Copy, Clone)]
pub enum Node<'a> {
    StrLit(Name),
    NumLit(u64),                       // f64 bit pattern; compare and hash by bits
    BoolLit(bool),
    Array(TypeId),
    Union(&'a [TypeId]),               // sorted by TypeId, deduplicated, len >= 2
    Intersection(&'a [TypeId]),        // written order, deduplicated by id, len >= 2; never sorted (1.13)
    Object(&'a ObjectShape<'a>),
    Function(&'a FnShape<'a>),
    Param(TypeParamKey),               // generic type parameter
    Ref(DeclSlot),                     // lazy named type (interface, class, enum)
    Named(DeclSlot, TypeId),           // alias wrapper; transparent in relations (1.8)
    App(DeclSlot, &'a [TypeId]),       // generic instantiation
}

pub struct ObjectShape<'a> { pub props: &'a [Prop], pub flags: ObjectFlags }

#[derive(Copy, Clone, PartialEq, Eq)]
pub struct Prop { pub name: Name, pub ty: TypeId, pub optional: bool, pub is_method: bool }

pub struct FnShape<'a> {
    pub params: &'a [ParamInfo],
    pub ret: TypeId,
    pub type_params: &'a [TypeParamKey],
    pub is_untyped: bool,
}

#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct Name(u32);                  // interned string (2.4)
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct DeclSlot(u32);              // index into Arena::decls
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct TypeParamKey { pub decl: DeclKey, pub index: u32 }
```

Every field is `Copy` or a bump reference, so `Node` needs no `Drop` (checked at compile time in 2.2). `ParamInfo` mirrors the current `Param` (type, optional, rest) but keeps the parameter name as a `Name`.

`TypeParamKey` replaces the current source-span key. A span is stable only within one parse; a declaration key plus index is stable across files and edits.

### 1.3 Arena structure

```rust
pub struct Arena<'a> {
    bump: &'a Bump,
    nodes: Vec<Node<'a>>,
    intern: FxHashMap<u64, SmallVec<[TypeId; 1]>>,   // digest -> candidate ids, verified on hit
    decls: Vec<DeclState<'a>>,
    decl_by_key: FxHashMap<DeclKey, DeclSlot>,
    names: &'a NameTable,                            // shared, read mostly (2.4)
    provider: &'a dyn DeclProvider,                  // fetches imported declarations (3.5)
    stable: Vec<Option<u128>>,                       // lazily computed stable hashes, indexed by slot
    inst_cache: FxHashMap<TypeId, TypeId>,           // App node -> expanded body
    union_display: FxHashMap<TypeId, &'a [TypeId]>,  // first-writer-wins as-written order
    limits: Limits,
    counters: Counters,
}
```

`nodes` is a `Vec` owned by the arena struct (a worker-lifetime object that is cleared on reset); the *contents* it points at live in the bump. If you prefer zero `Vec` growth, a bump-backed `bumpalo::collections::Vec` also works. Both are fine because `Node` has no `Drop`.

### 1.4 Interning algorithm

All composite construction goes through one function that takes scratch input and returns an id.

```text
intern_object(scratch: &mut SmallVec<[Prop; 8]>, flags) -> TypeId:
  sort scratch by name                              // canonical order, stable sort
  digest = fx_hash(OBJECT_TAG, flags, scratch)      // shallow: children are ids
  for cand in intern[digest]:
      if nodes[cand] is Object and props equal scratch (elementwise) -> return cand
  slice = bump.alloc_slice_copy(scratch)            // Prop is Copy
  shape = bump.alloc(ObjectShape { props: slice, flags })
  id = push_node(Node::Object(shape))               // checks limits, may return Err(Budget)
  intern[digest].push(id)
  return id
```

- The digest is only a bucket key. A hit is always verified by comparing contents, so a collision costs one extra comparison and can never merge different types.
- Probing happens **before** allocating into the bump, so duplicate shapes do not waste arena bytes.
- Shallow hashing treats child `TypeId`s as opaque numbers. This is sound because every child id was itself produced by the same arena.
- `Union`, `Array`, `Function`, literals and `App` follow the same pattern with their own tag.
- `Ref` and `Named` are **not** interned by content: a `Ref` is created once per declaration slot and cached in `DeclState`.

### 1.5 Object shapes

- Properties are sorted by `Name` text, not by `Name` number, so order does not depend on the order names were interned. Compare through `NameTable::text(name)`. (The merge-join in subtyping relies on this order, as it does today.)
- Build into a `SmallVec` on the stack, then copy into the bump (`alloc_slice_copy`). **Never** store the `SmallVec` itself in a node (it can spill to the heap, and the spill would never be freed).
- Source order for display lives outside the node, in diagnostics or a side table, never in the identity.

### 1.6 Unions

```text
make_union(members) -> TypeId:
  flatten nested unions; drop NEVER; if any member is ANY -> ANY (current behavior kept)
  deduplicate by TypeId (identity is canonical inside one arena)
  sort by TypeId (a total order within this arena)
  0 members -> NEVER; 1 member -> that member
  digest = fx_hash(UNION_TAG, members)
  probe and verify; else alloc_slice_copy and push
  record union_display[id] = as-written order, first writer wins
```

- Today's `structurally_equal` fallback during dedup exists because named copies have distinct ids. With alias wrappers and `Ref` nodes (1.7, 1.8) identity is canonical, so dedup by `TypeId` is enough. Keep `structurally_equal` as a debug assertion until fixtures confirm.
- Order for identity is by `TypeId`, which depends on creation order *within one arena*. It must **never** be used for the stable hash, which sorts by member stable hashes (3.2).
- `union_display` keeps messages in source order. First-writer-wins is a known, accepted limitation (HLD open decision 3).

### 1.7 Lazy declaration slots (replaces placeholders and `set()`)

```rust
pub enum DeclState<'a> {
    Unresolved(DeclSource<'a>),         // AST reference for local declarations
    Resolving,
    Resolved { body: TypeId },          // body is an Object/Function/etc. node
    Imported(DeclKey),                  // body fetched on first expand via the provider
    Failed,                             // resolution error already reported
}
```

State machine:

```text
Unresolved --expand()--> Resolving --success--> Resolved
                              |--cycle detected--> Resolved (body refers back to the Ref node; fine)
Imported   --expand()--> provider.fetch(key) --> rebuild --> Resolved
```

- The type of an interface or class is the id of its `Ref(slot)` node. It is created immediately and never changes.
- `expand(id)` returns the body: for `Ref(slot)` it resolves the slot if needed and returns `body`; for `Named(_, inner)` it returns `expand(inner)`; for `App` it consults `inst_cache` or instantiates; other nodes return themselves.
- Self reference (`interface Node { next: Node }`): while the slot is `Resolving`, the field type is the `Ref(slot)` node, which is valid immediately. No placeholder object is mutated, so nothing needs invalidating afterwards.
- Relations (subtyping, equality) call `expand` on both sides before structural comparison and keep the `seen` stack they already use for coinduction.

**What disappears from today's arena:** `alloc_object_placeholder`, `set`, `make_unique`, `duplicate_named`, the clearing of `param_scan` and `equality_cache` inside `set`, and the debug asserts that exist to prevent renaming shared ids. Caches keyed by `TypeId` become append-only and need no invalidation.

### 1.8 Aliases and display names

`type Scores = number[]` produces `Named(slot, array_id)`.

- It is a distinct id from the plain `number[]`, so naming it cannot rename other uses of `number[]`.
- Relations peel it with `expand`, so it behaves as the underlying type. The `==` fast path only misses, which falls back to structural comparison (correct, slightly slower).
- A display name is simply the name of the slot. There is no `display_names` side table keyed by id and no uniqueness rule to enforce.
- Generic instantiations print from `App(slot, args)` directly (name plus printed arguments), replacing the baked-in text names such as `Box<Dog>` and the `has_settled_display` workaround.

`Record<K, V>` and `Promise<T>`: model as `App` of a built-in declaration slot (library declarations, 7.3) so no `record_value_types` side table is needed.

### 1.9 Generics

- Declarations with type parameters record their `TypeParamKey`s in `DeclState` or `FnShape`.
- `App(slot, args)` is interned like any composite. Expansion substitutes `args` into the body and caches the result in `inst_cache` (keyed by the `App` id). The cache is local to the arena and append-only.
- The existing substitution and inference code (`semantic/generics.rs`) keeps its algorithms; it changes in two ways: parameters are identified by `TypeParamKey`, and "does this type mention a type parameter" becomes an append-only per-id byte vector (answers can no longer be invalidated because nodes are immutable).
- Constraint checking during resolution can use the relation cache safely, because a `Ref` is valid before its body exists (the current code avoids the cache here to dodge empty placeholders).

### 1.10 Caches inside the arena

| Cache | Key | Invalidation | Notes |
|---|---|---|---|
| Relation cache (subtype, disjoint) | (relation, source id, target id) | never within a unit | Dropped on reset |
| Instantiation cache | `App` id | never | Dropped on reset |
| Mentions-type-parameter | id (vector index) | never | One byte per id |
| Stable hash | id (vector index) | never | Lazy (3.2) |
| Union display | union id | never | First writer wins |

### 1.11 Reset protocol and session safety

```rust
pub struct Worker { bump: Bump /* reused */ }

impl Worker {
    pub fn run_unit<R>(&mut self, ctx: &UnitContext, f: impl FnOnce(&mut Arena<'_>) -> R) -> R
    where R: 'static + Send                       // owned output only
    {
        let out = {
            let mut arena = Arena::new(&self.bump, ctx);
            f(&mut arena)
        };                                        // arena and all borrows end here
        self.bump.reset();                        // needs &mut Bump: borrow checker proves no live references
        out
    }
}
```

- The `R: 'static + Send` bound means the unit's result cannot borrow from the arena. A `TypeId` could still sneak out as a bare `u32` newtype; to prevent this, make `TypeId` carry a phantom lifetime tied to the arena (`TypeId<'a>`) in the public checker API, or run a lint in review that forbids `TypeId` in any `Output` struct. Decide when stage 1 starts.
- The borrow rules make "reset while references exist" a compile error. This is the main safety benefit of using `&mut Bump` for reset.
- `Bump::reset` keeps the largest chunk and frees the rest [VERIFY], so memory use after a huge file does not stay at its peak. Consider dropping and recreating the bump if one unit exceeded a size threshold.

### 1.12 Invariants and tests (arena)

| Invariant | Test |
|---|---|
| Interning is idempotent | property test: building the same shape twice yields one id |
| Object property order never affects identity | property test: shuffle input order |
| Union identity is order independent | property test: permute members |
| Union display order preserved | unit test with first-writer-wins |
| `expand` never loops | recursive interface, mutually recursive aliases, recursive generics |
| No `Drop` types in nodes | compile-time assertion (2.2) |
| Reset leaves no live references | compile-fail test (a reference kept across `run_unit` must not compile) |
| Intersection identity is ordered | unit test: `A & B` and `B & A` are two ids and mutually assignable |
| Intersection construction never reads a body | unit test: build with an unresolved `Ref` member, resolve it, and the result is unchanged |
| Every intersection comes from `alloc_intersection` | substitution test: `T & number` with `T := string` is `never` |

### 1.13 Intersections: canonical form (decided before the node exists)

Intersections touch interning (1.4), unresolved declarations (1.7), generics (1.9) and the stable hash (3.2) at once, so the rules are fixed here first. Each was checked against tsc 5.9.3 with a probe over the compiler API; the table at the end records what it printed.

**The node.** `Intersection(&'a [TypeId])`: members in written order, deduplicated by id, at least two. It is not sorted. A union is sorted because its members are alternatives; an intersection's order carries meaning.

**Rule 1: identity is ordered.** `A & B` and `B & A` are two ids that are mutually assignable. tsc does the same. Order cannot be dropped, because the call signatures of the members are tried in member order: `((x: any) => string) & ((x: number) => number)` called with `1` returns `string`, and the reverse order returns `number`. Sorting would be safe only when no member is callable, and whether a member is callable needs its body, which may not exist yet. An identity that follows the written order never has to ask.

**Rule 2: reduction happens at two levels, and only the first is identity.**

*Construction*, in `alloc_intersection`. It reads only what an id and its wrappers show: the node kind, primitive and literal facts, and the contents of a `Named` or `App` wrapper. It never looks through a `Ref`. In order:

1. A `never` member gives `never`. This is checked first, because `any & never` is `never`.
2. Flatten: a member that is an intersection, seen through `Named` and `App` wrappers, is replaced by its members in place. A nested alias therefore prints flat.
3. Remove `unknown`. If nothing remains the result is `unknown`.
4. An `any` member gives `any`.
5. Primitives and literals (`void` is kept next to an object, `void & A`, and cancels only against another primitive, as in tsc 5.9.3): two different primitive kinds give `never`; a literal with its own primitive gives the literal; two different literals give `never`; `null` or `undefined` with any member that is not itself gives `never`. An object-like member never cancels a primitive, so `string & { __brand: "x" }` stays an intersection.
6. Distribute: if a member, seen through wrappers, is a union, the result is the union of the intersections of each choice, in written order. Each choice is built by this same function, so `("a" | "b" | "c") & string` reduces choice by choice. If the product of the union sizes reaches 100,000 (a `never` factor counts as zero) the result is the error type and the diagnostic is TS2590, as in tsc.
7. Drop an anonymous `{}` (an `Object` with no properties) when another object-like member is present.
8. Deduplicate by id, keeping the first occurrence. Never structurally: structural equality reads bodies.
9. No members gives `unknown`, one member gives that member, otherwise the ordered list is interned.

*Reduction*, in `reduced(id)`. It reads bodies, runs when a relation or a member access asks, and is cached per `(id, generation)` like any relation answer. What it decides is never written back into the intersection's identity:

- A discriminant conflict (`{ kind: "a" } & { kind: "b" }`) reduces to `never`. tsc keeps the intersection node and reduces it on demand, which is why it prints as `never` while its flags still say intersection.
- A property that appears in several members has the intersection of their types, built with `alloc_intersection`; `{ x: number } & { x: string }` has `x: never`.
- The signatures of callable members are concatenated in member order.

**Why the line is here.** Construction runs while declarations are still being resolved, when a `Ref` reads as an empty object. Any decision that needed a body would bake an answer about that empty object into the intern table, which is the old placeholder bug again, and it would be cached under a stable id where nothing invalidates it. So construction uses ids and wrappers only, and everything body-aware is lazy and generation-scoped.

**Interning.** An intersection is interned by its ordered member ids, after construction. Anonymous members are hash-consed first, so two mentions of `{ a: 1 } & { b: 2 }` are one id; a `Ref` member is unique per declaration. Normalizing before interning means the table never holds a form that construction would have reduced.

**Rule 3: one door.** Every intersection comes from `alloc_intersection`. Substitution rebuilds through it (`T & number` with `T := string` must be `never`), and so do narrowing and spread. No other code allocates the node.

**Rule 4: hashing.** The encoding in 3.2 hashes the members in order, not sorted. Reordering the members of a written intersection therefore changes the `SigHash`. That is an extra recheck and never a stale result; sorting would be unsound for overloads.

**Rule 5: display.** Members print in written order, a `Named` member prints its name, and no first-writer table is needed because identity is ordered.

**Relations (for when they are written).** A source is assignable to an intersection when it is assignable to every member. An intersection is assignable to a target when some member is, or, for an object target, when the merged properties of the members satisfy it. Equality is structural, so `A & B` equals `B & A` as a relation; only call-signature order tells them apart.

**Observed in tsc 5.9.3**

| Input | Result | Rule |
|---|---|---|
| `A & B`, `B & A` | two ids, assignable both ways, members in written order | 1 |
| `AB & C` with `type AB = A & B` | members `A, B, C`, the same id as `A & B & C` | construction 2 |
| `A & A & B` | members `A, B`, the same id as `A & B` | construction 8 |
| `string & number`, `"a" & "b"`, `null & string` | `never` | construction 5 |
| `"a" & string`, `1 & number`, `true & boolean` | the literal | construction 5 |
| `string & { __brand: "x" }` | stays an intersection | construction 5 |
| `number & string & { z: 1 }` | `never` | construction 5 |
| `A & unknown`, `unknown & unknown` | `A`, `unknown` | construction 3 |
| `A & never`, `any & never` | `never` | construction 1 |
| `any & A` | `any` | construction 4 |
| `{} & A`, `A & {}` | `A`, the same id as `A` | construction 7 |
| `object & A` | stays an intersection | not reduced |
| `(A \| B) & C` | `A & C \| B & C`, written order inside each | construction 6 |
| `("a" \| "b") & ("b" \| "c")` | `"b"` | construction 6 |
| `{ kind: "a" } & { kind: "b" }` | an intersection node that prints as `never` | reduction |
| `{ x: number } & { x: string }` | property `x` has type `never` | reduction |
| callable members in either order | the first matching signature wins | 1 |
| a product of union sizes of 100,000 or more | error TS2590, "too complex to represent" | construction 6 |

**Open ([VERIFY]).** The 100,000 cap is tsc's; tsox's own budget (ADR-6) may set it lower after measurement. Rules 5 and 7 are written for the strict mode this checker runs in and for anonymous `{}` only; an empty interface is a `Ref` and is not dropped. Type parameters stay opaque members (`T & U` is kept), and the `T & {}` form used by `NonNullable<T>` needs its own case when generics reach it. Enum literal members are treated as literals through their `Named` wrapper and need a fixture.

---

## 2. Memory (ADR-2)

### 2.1 Rules (reviewable as a checklist)

1. Anything allocated in a bump is `Copy` or a reference to bump data. No `Vec`, `String`, `Rc`, `Arc` or `Box` inside bump nodes.
2. Scratch collections (`SmallVec`) live on the stack and are copied into the bump when final.
3. Anything leaving a unit is owned data in an `Arc`, with no arena pointers and no `TypeId`.
4. Cycles in persistent data are expressed with indices, not pointers.
5. `Rc` is for thread-confined state only; crossing a thread boundary requires `Arc` or an owned message.
6. Small ids are plain `Copy` values; `Arc` is for large payloads (shapes, strings, export tables), to keep reference-count traffic off hot paths.

### 2.2 Compile-time guard

```rust
const _: () = assert!(!std::mem::needs_drop::<Node<'static>>());
const _: () = assert!(!std::mem::needs_drop::<Prop>());
const _: () = assert!(!std::mem::needs_drop::<ParamInfo>());
```

These fail the build if a field that needs `Drop` is ever added.

### 2.3 Allocator

```rust
#[cfg(not(target_arch = "wasm32"))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;
```

- Native only. WASM keeps the default allocator and its separate build profile.
- Compare benchmark results only between runs with the same allocator. dhat sees allocations that go through the global allocator, not the bump's chunk allocations, so also count bump bytes (2.5).

### 2.4 Name interning

- A global `NameTable` maps text to `Name(u32)` and back. Reads dominate, so use a sharded concurrent map plus a append-only text store, or a ready-made threaded interner (for example `lasso`'s threaded variant) [VERIFY].
- `Name` numbers depend on interning order, which varies with thread timing. **Therefore `Name` numbers must never influence output order or stable hashes.** Sort and hash by text.

### 2.5 Budgets

```rust
pub struct Limits {
    pub max_nodes_per_unit: u32,        // e.g. 4_000_000, tune with data
    pub max_bump_bytes: usize,
    pub max_instantiation_depth: u16,
    pub max_union_members: u32,
}
```

- `push_node` and `alloc_slice_*` return `Err(Budget)` when a limit is hit. The checker aborts the unit, emits a single diagnostic naming the limit, and returns `UnitOutput { incomplete: true, .. }`.
- `incomplete` results are carried to the query layer (6.3) so they are never treated as proof that a file is clean.
- Counters (nodes, bump bytes, instantiation count, relation cache size) are recorded per unit for the measurement harness.

### 2.6 Diagnostics ownership

Diagnostics are rendered to `String` **inside** the unit, while the arena is alive (type printing needs it). They contain file id, byte span, code and message text only.

---

## 3. Stable identity and portable types (ADR-5)

### 3.1 Keys

```rust
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct ModuleKey(u128);   // hash of the module's identity, by the rule in 3.1.1

#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct DeclKey(u128);     // hash(ENC_VERSION, ModuleKey, local name, disambiguator)

#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct SigHash(u128);     // hash of the declaration's own public shape (3.3)
```

- `disambiguator` distinguishes merged declarations (interface + namespace + function with the same name) and overloads: use a stable kind tag plus an index in source order within that merge group.
- Your earlier "chunk id" is the pair (`DeclKey`, `SigHash`); references between declarations carry the `DeclKey` only (HLD ADR-5).
- A key is never built from a `FileId`. `FileId` is a run-local index (4.1) and changes when a file is added.

#### 3.1.1 Module key rule

```text
ModuleKey = XXH3-128( ENC_VERSION:u8 || kind:u8 || fields )
  0x01 Project  : path
  0x02 Package  : name || version || subpath
  0x03 External : path
  each field    : len:u32le || utf8 bytes
```

- **Project:** a file under the project root whose canonical path does not pass through a `node_modules` directory. `path` is relative to the root, with `/` separators.
- **Package:** a file whose canonical path passes through `node_modules`. Its package is the nearest `package.json` at or above its directory, searching upward but not past `node_modules`, that has both a `name` and a `version`. `name` is taken verbatim, scope included. `subpath` is relative to the package directory, with `/` separators.
- **External:** every other file, including one inside `node_modules` with no such `package.json`. `path` is the absolute canonical path, with `/` separators.
- A `package.json` without a `name` or without a `version` (a `{"type": "module"}` marker, for example) defines no package, and the search continues upward.
- Workspace packages and `npm link` targets canonicalize to a path outside `node_modules`, so they are Project or External files.
- **Never in a key:** a `FileId`, the absolute project root, a modification time, or anything from the host environment.
- **Why:** an id that survives moving the project directory, installing it at another path, or pnpm's symlinked layout is the point of having a persistent id. A package is named for what it is, not for where it was installed.
- **Cost:** two copies of the same name and version with different contents, one patched in place, get the same key. That is rare, and it is stated here so it is not discovered later.
- **Root:** `ModuleGraph` has no project root today. Stage 2 adds it as an input of the graph.
- **[VERIFY]** On a case-insensitive filesystem the key uses whatever casing `canonicalize` returns. A stage 2 test per platform confirms that two spellings of one file give one key.

### 3.2 Canonical encoding and the stable hasher

The hash input is an explicit byte stream, never `#[derive(Hash)]` (the derive's layout is not a stable format).

```text
ENC_VERSION: u8 = 1   (first byte of every stream; bump on any change)

node encoding = tag:u8 || payload
  0x01..0x0A  primitives (number .. void), no payload
  0x10 StrLit    : len:u32le || utf8 bytes
  0x11 NumLit    : bits:u64le
  0x12 BoolLit   : u8
  0x20 Array     : child hash128
  0x21 Union     : n:u32le || n child hash128, sorted ascending by hash128
  0x22 Object    : flags:u8 || n:u32le || n * (name_len:u32le || name bytes || optional:u8 || is_method:u8 || child hash128), props sorted by name bytes
  0x23 Function  : n_params:u32le || n * (optional:u8 || rest:u8 || child hash128) || ret hash128 || n_tparams:u32le || tparam keys
  0x24 Intersection : n:u32le || n child hash128, in member order, NOT sorted (1.13)
  0x30 DeclRef   : DeclKey (16 bytes le)           // named reference: identity only
  0x31 App       : DeclKey || n:u32le || n child hash128
  0x32 TypeParam : DeclKey || index:u32le
  0x33 Named     : (not hashed; peel to inner)      // aliases are transparent for identity
```

- **Merkle style:** each node's hash is `H(encoding)` where children contribute their own 128-bit hashes. Hashes are cached per `TypeId` in `Arena::stable` (lazy: only computed for types that reach an export boundary).
- **Hasher:** XXH3-128 (streaming). Non-cryptographic; adequate against accidental collisions. If untrusted source must be assumed adversarial, use a truncated cryptographic hash behind a feature flag.
- **Order independence:** unions sort by child hash, objects by property name bytes. Arena creation order, `Name` numbers and thread timing never appear in the stream.
- **No cycles to handle:** structural encoding stops at `DeclRef`/`App`/`TypeParam` (3.3).

#### 3.2.1 Amendments made while building 5a

- `Function` carries `is_untyped:u8` straight after the tag, before `n_params`. An untyped function accepts any call, so it is part of the type's identity. `Error` takes primitive tag `0x0A`.
- `ModuleKey::builtin()` (kind `0x04`, no fields) is the module of the built-in library declarations, so `Promise` and `Array` have a `DeclKey` without a file: `DeclKey::builtin(name)`.
- A `DeclKey` stream is `ENC_VERSION || 0xDE || ModuleKey || name || kind:u8 || index:u32le`. `DeclKey::child(owner, ordinal)` (`0xDC`) names something that owns type parameters without being a declaration, such as the second generic function type inside one alias, so `TypeParam(owner, index)` stays unique with no wire change.
- A `FunctionType` has no type-parameter list; the declaration that owns it has (`PortableDecl::type_params`), so the `tparams` field of a `Function` node is empty for now.
- `ModuleGraph::module_key` gives Project and External keys. A file under `node_modules` has no key until the Package rule (the `package.json` walk) is written.

#### 3.4.1 Amendments made while building 5b and 5c

- **Names travel, parameter names are not identity.** `types.rs` says structural equality ignores a parameter's name, so the identity hash leaves it out. Messages print it, so `PParam` carries it, and the export de-duplicates nodes with a second hash that includes names (two functions that differ only in parameter names stay two nodes). A parameter rename therefore changes the `PortableType` but not the `SigHash`.
- **`DeclRef`, `App` and `TypeParam` carry a name** (and a type parameter its bound), so an importer can print them without fetching the declaration.
- **`App` also carries its instantiated body.** The checker's `App` holds one, and rebuilding it would need substitution over a declaration that has no AST in the importing unit.
- **An `App` imports with its own name slot** (`alloc_name`), as `type_annotation.rs` does. `Promise` and `Record` use the shared built-in slot.
- **Aliases are transparent in the portable body** and the importer puts the name back from the declaration's kind (`DeclKind::Alias`).
- **Import is eager over what is reachable, not lazy** (3.5 sketched lazy expansion): `TypeArena::get` takes `&self`, so it cannot fetch. The importer drains a pending list, and a key already seen is never queued twice, so a cycle ends. Fine-grained dependency edges are therefore per declaration reached from an import, not per use.
- **`SigHash` includes type parameter bounds** and the kind; the name is already in the `DeclKey`.
- **Not portable yet:** enums, namespaces, and the value side of a class. `export_decl` returns None for a body that mentions a declaration or type parameter with no key, and an import of a declaration the provider lacks reads as `Error`, which does not cascade.

### 3.3 Signature hash

`SigHash` of a declaration = hash of the encoding of its **own** public shape, where every reference to another declaration is a `DeclRef`/`App` (identity only). It does not include the referenced declaration's shape.

What counts as public shape:

| Declaration | Included | Excluded |
|---|---|---|
| Function | name, type parameters and constraints, parameter types and optionality, return type (declared, or inferred type if unannotated) | body |
| Interface / type alias | members, heritage references, type parameters | comments |
| Class | instance and static members, visibility modifiers, heritage, constructor signatures | method bodies, private member types as needed for compatibility (decide per language rules) |
| Variable / const | declared or inferred type | initializer expression |
| Enum | member names and values | none |

Consequence: a body-only change leaves `SigHash` unchanged, so early cutoff (6.3) stops the query layer from rechecking importers.

**Inferred types:** if a declaration has no annotation, its type is inferred from its initializer or body, so its `SigHash` can change when the body changes. That is correct (the exported type did change) and is the reason shape and bodies are separated in the unit (4.4).

### 3.4 Portable types

```rust
pub struct PortableDecl {
    pub key: DeclKey,
    pub sig: SigHash,
    pub kind: DeclKind,                   // Interface, Class, Alias, Function, Var, Enum, Namespace
    pub type_params: Box<[PTypeParam]>,
    pub body: PortableType,
}

#[derive(PartialEq, Eq, Hash, Clone)]
pub struct PortableType { pub nodes: Box<[PNode]>, pub root: u32 }   // indices into nodes

#[derive(PartialEq, Eq, Hash, Clone)]
pub enum PNode {
    Prim(PrimKind),
    StrLit(Box<str>), NumLit(u64), BoolLit(bool),
    Array(u32), Union(Box<[u32]>),
    Object { props: Box<[PProp]>, flags: u8 },
    Function { params: Box<[PParam]>, ret: u32, tparams: Box<[PTypeParam]> },
    DeclRef(DeclKey), App(DeclKey, Box<[u32]>), TypeParam(DeclKey, u32),
}
```

- Owned, arena independent, `Eq + Hash` by content. This is what Salsa compares to decide whether an output changed.
- **Deterministic serialization:** nodes are emitted by a depth-first walk with children visited in canonical order (properties sorted by name, union members sorted by stable hash). Two equal types therefore serialize identically, so derived `Eq` on `PortableType` is meaningful.
- Shared substructure appears once (DAG, by node index) to avoid exponential blowup; the walk memoizes by arena `TypeId`.
- Exporting: `arena.export_decl(slot) -> PortableDecl` expands the declaration's body once, emitting references to other declarations as `DeclRef` without expanding them.

### 3.5 Materialization (portable to local)

```rust
pub trait DeclProvider {
    /// Returns the declaration, recording a query dependency on it [VERIFY wiring in 6.2].
    fn decl(&self, key: DeclKey) -> Option<Arc<PortableDecl>>;
}

impl<'a> Arena<'a> {
    fn slot_for_key(&mut self, key: DeclKey) -> DeclSlot;            // creates DeclState::Imported(key) on first use
    fn import_type(&mut self, p: &PortableType, memo: &mut [Option<TypeId>]) -> TypeId;
}
```

Algorithm for `expand` of an `Imported(key)` slot:

```text
d = provider.decl(key)           // dependency edge recorded here
memo = vec![None; d.body.nodes.len()]
body = import_type(&d.body, &mut memo)
state = Resolved { body }
```

`import_type` walks the node vector (iteratively, to avoid deep recursion on long chains), interning each node bottom-up; `DeclRef(k)` becomes the `Ref(slot_for_key(k))` node and is **not** expanded, so cyclic references terminate naturally.

Properties of this design:
- Rebuilding is lazy: only declarations actually touched by checking are fetched.
- The dependency edge is recorded at fetch time, so a unit depends on exactly the declarations it read (fine-grained invalidation).
- A per-arena map from `DeclKey` to slot guarantees one slot per declaration, so two uses of the same imported type share ids.

### 3.6 Versioning and collisions

- `ENC_VERSION` is part of every key. Any change to the encoding bumps it, which invalidates stored fingerprints (if persisted) rather than mixing formats.
- Collision policy: treat 128-bit equality as identity. As a debug-only check, a test mode can keep a map from hash to canonical encoding and assert equal encodings for equal hashes.

### 3.7 Tests (identity)

| Test | What it proves |
|---|---|
| Hash is the same under different arena creation orders | creation order does not leak |
| Hash is the same across process runs | no random seeds |
| Hash is the same before and after a reset and rebuild | no pointer or slot dependence |
| Property or union order permutations give the same hash | canonical ordering works |
| Reordering the members of an intersection changes the hash | order is identity for intersections (1.13) |
| Export, import, export round trip gives the same hash | materialize is faithful |
| Body-only edit keeps `SigHash`; signature edit changes it | cutoff behavior |
| Recursive interface hashes without recursion | cycle breaking at `DeclRef` |
| Golden file of hashes for a fixture set | detects accidental encoding changes |

---

## 4. Module graph and units (ADR-3, ADR-4)

### 4.1 Data structures

```rust
pub struct ModuleGraph {
    pub files: Vec<FileId>,                       // sorted by canonical path; index = deterministic order
    pub deps: Vec<Box<[Edge]>>,                   // forward edges: importer -> imported
    pub rdeps: Vec<Box<[FileId]>>,                // reverse edges
    pub scc_of: Vec<SccId>,
    pub sccs: Vec<Scc>,                           // members sorted by FileId
    pub topo: Vec<SccId>,                         // dependencies first
}
pub struct Edge { pub to: FileId, pub kind: EdgeKind }       // bit flags below
bitflags! { pub struct EdgeKind: u8 { const VALUE=1; const TYPE_ONLY=2; const SIDE_EFFECT=4; const DYNAMIC=8; const REEXPORT=16; } }
```

- Build a `petgraph::DiGraph<FileId, EdgeKind>` from resolved imports, run `tarjan_scc`, then derive `scc_of`, `sccs` and `topo` into the flat vectors above. The flat vectors are what the scheduler reads.
- petgraph returns components in an order where imported modules come before importers for this edge direction [VERIFY with a test]. Do not rely on it implicitly: assert the order in a unit test and derive `topo` explicitly.
- Sort files by canonical path first, so `FileId` assignment is deterministic.

### 4.2 Edge kinds

| Kind | Creates ordering dependency | Notes |
|---|---|---|
| value import / export | yes | |
| type-only import | yes (types are needed) | |
| side-effect import | no type dependency | kept for graph reachability |
| dynamic import | only if its type is used (`typeof import(...)`) | otherwise reachability only |
| re-export (`export *`, `export { x } from`) | yes | `export *` cycles need a visited set when computing export names |

### 4.3 Unit of work

A **unit** is one SCC. One worker checks it in one arena, so members of a cycle can reference each other's declarations through shared `DeclSlot`s without any cross-arena rebuilding.

Intra-unit protocol:

```text
check_unit(scc):
  phase A (declare): for each member (FileId order): parse; register every declaration as a slot
                     and every import binding (ready to resolve, nothing expanded yet)
  phase B (shapes):  for each declaration that is exported: resolve its type (expand lazily);
                     annotated declarations resolve from syntax; unannotated need checking of
                     the initializer or body (4.4)
  phase C (bodies):  check bodies and statements per file in FileId order; emit diagnostics
  publish:           UnitOutput { exports (portable), diagnostics, incomplete }
```

### 4.4 Inferred exports and declaration cycles (the hardest correctness point)

Problem: an unannotated export like `export const x = f()` has a type that depends on checking `f`'s result, which may live in another module in the same cycle or even depend back on `x`.

Design:
- Resolution of a declaration's type is **on demand** through its slot (`Resolving` state). If resolution re-enters a slot already `Resolving` for a *value* inferred type, that is a genuine circularity; report the equivalent of tsc's implicit-any circularity error, and use `ERROR` for that edge (documented behavior, same as tsc).
- Annotated declarations never trigger body checking when their type is needed. Make this the fast path: it lets downstream units start without waiting for the upstream bodies phase, if the scheduler publishes shapes at the end of phase B. (v1 publishes once at the end; splitting publication is an optimization noted in HLD section 4, item 1.)
- Cycle between two units cannot occur (units are SCCs of the module graph). A *type-level* cycle inside one unit is handled by the slot state machine.

### 4.5 Graph updates

- File added or removed, or an import list changed: rebuild the graph (cheap: edges only) and recompute SCCs. A body-only edit does not change edges, so the graph query result is unchanged and downstream queries stay green (6.3).
- `rdeps` gives the transitive closure for diagnostics like "what is affected", but **invalidation itself is done by the query layer**, not by walking `rdeps`.

---

## 5. Concurrency (ADR-3)

### 5.1 Pipeline overview

```text
Stage 1  (parallel, bounded)         Stage 2  (scheduled by readiness)
read + hash + header  ──channel──►  graph build ──► scheduler ──► workers (each: Bump + checker)
                       reorder                                    │
                                                                   ▼
                                                           ordered publication
```

### 5.2 Stage 1: read, hash, header

- Run on a rayon pool (`par_iter` over the sorted file list or `scope` with spawns).
- Each task: read bytes (or take from the editor buffer), compute XXH3-64 for the file guard (6.1), parse inside a task-local bump, extract a `ModuleHeader` (import and export specifiers, declaration keys, source hash), drop the AST.
- Output message: `HeaderMsg { index: usize, file: FileId, header: ModuleHeader }` (all owned) sent on `crossbeam_channel::bounded(cap)` with `cap = 2 * workers`. Backpressure stops stage 1 from running far ahead.
- A reorder buffer on the receiving side holds out-of-order messages in a `BTreeMap<usize, HeaderMsg>` and releases them in index order. Everything downstream sees files in the same order every run.
- **Syntax trees do not cross threads in v1.** Checking re-parses the file in the worker that checks it. This costs a second parse per file but avoids self-referential structures (an AST bundled with its own allocator). If measurements show parse cost matters, the alternative is to keep the parsed unit on the thread that will check it and schedule on that basis; evaluate after S3.
- Resolution of import specifiers (the resolver) runs here or lazily at graph build; its results become query inputs (6.4).

### 5.3 Stage 2: scheduler

```rust
struct Scheduler {
    graph: Arc<ModuleGraph>,
    pending: Vec<AtomicU32>,                       // remaining un-published dependencies per SCC
    ready: crossbeam_deque::Injector<SccId>,       // or a Mutex<BinaryHeap> keyed for determinism of tie-breaks
    results: Vec<OnceLock<Arc<UnitOutput>>>,       // index = SccId, written exactly once
    remaining: AtomicUsize,
    cancel: AtomicBool,
}
```

Worker loop:

```text
loop:
  if cancel or remaining == 0: break
  scc = ready.steal() or park briefly
  out = catch_unwind(|| worker.run_unit(ctx_for(scc), |arena| check_unit(arena, scc)))
        on panic: synthesize UnitOutput::crashed(scc, panic message)   // other units unaffected
  results[scc].set(Arc::new(out))
  for dependent in rdeps_scc(scc):
      if pending[dependent].fetch_sub(1) == 1: ready.push(dependent)
  remaining -= 1
```

- Initial ready set: SCCs with zero dependencies, pushed in `topo` order.
- Which worker runs which unit may vary between runs; **results do not**, because units only read published, immutable outputs of their dependencies.
- Publication to the user happens after all units finish (or incrementally in `topo` order for editor use): iterate `topo`/file order, concatenate diagnostics, sort within file.

### 5.4 Worker state

- One `Worker { bump: Bump }` per rayon thread (thread-local or owned by the pool). `Bump` is `Send` but not `Sync`, so it cannot be shared by accident.
- Immutable shared inputs per unit (`UnitContext`): `Arc<ModuleGraph>`, `Arc` of dependency outputs, `&NameTable`, a `&dyn DeclProvider` that looks up dependency outputs by `ModuleKey`.
- Relation caches and instantiation caches are inside the arena (per unit), so there is nothing to lock.

### 5.5 Cancellation and failure

- `cancel` is checked at unit start and every N statements in the checker. A cancelled unit returns no output and is never cached.
- Panics are contained per unit (above). A crashed unit produces an internal-error diagnostic and marks dependents as depending on a crashed output (they still run; its exports are treated as unknown/`ERROR`).
- The determinism test (9.3) runs with injected random delays and varying thread counts and requires byte-identical results.

### 5.6 Where parallelism will not help

- One large SCC runs on one thread (HLD risk 2). The harness must report the largest SCC size and time.
- Phase B/C of a unit are sequential inside the unit in v1.

---

## 6. Query layer (ADR-4)

### 6.1 File guard (front of the pipeline)

```text
on file event:
  if (mtime, size) == cached: stop
  hash = xxh3_64(bytes)
  if hash == cached_hash: update stored mtime/size; stop          // saved without changes
  set SourceFile input (text, hash)                                // starts a new revision
```

For small files the metadata check dominates, so claims about hash throughput do not matter in practice; measure before optimizing.

### 6.2 Inputs and queries (sketch)

```rust
#[salsa::input] struct SourceFile { path: PathBuf, text: Arc<str>, hash: u64 }
#[salsa::input] struct Project   { files: Vec<SourceFile>, config: Arc<Config>, resolution_epoch: u64 }

#[salsa::tracked]
fn module_header(db: &dyn Db, f: SourceFile) -> Arc<ModuleHeader>;           // owned, Eq

#[salsa::tracked]
fn resolved_imports(db: &dyn Db, p: Project, f: SourceFile) -> Arc<[ResolvedImport]>;   // reads p.resolution_epoch

#[salsa::tracked]
fn module_graph(db: &dyn Db, p: Project) -> Arc<ModuleGraph>;                 // reads all resolved_imports

#[salsa::tracked]
fn check_scc(db: &dyn Db, p: Project, scc: SccKey) -> Arc<UnitOutput>;        // heavy; runs the checker

#[salsa::tracked]
fn decl_shape(db: &dyn Db, p: Project, module: ModuleKey, key: DeclKey) -> Option<Arc<PortableDecl>>;   // projection

#[salsa::tracked]
fn file_diagnostics(db: &dyn Db, p: Project, f: SourceFile) -> Arc<[Diagnostic]>;  // projection
```

- `SccKey` is a stable key (hash of the sorted member `ModuleKey`s), not a positional index, so an SCC that keeps the same members keeps its query identity across graph recomputation.
- `UnitOutput { exports: IndexMap<DeclKey, Arc<PortableDecl>>, diagnostics: BTreeMap<FileId, Arc<[Diagnostic]>>, incomplete: bool }` is `Eq`; map order is source/deterministic.
- **Projection queries** (`decl_shape`, `file_diagnostics`) read one entry out of a bigger output. If the entry is equal to its previous value, the projection is unchanged and everything that read it stays green. This gives per-declaration and per-file cutoff without per-declaration checking.
- `DeclProvider::decl` (3.5) is implemented by calling `decl_shape`, which is how dependency edges are recorded as the checker touches declarations.
- Do **not** create queries for types, expressions or relations.

**[VERIFY]** in spike S1: that outputs only need `Eq` (and clone cheaply via `Arc`), that backdating works as described for the pinned version, and how database handles are obtained for worker threads.

### 6.3 Early cutoff behavior and `incomplete`

- `module_header` is unchanged by a body edit (imports, export names and declaration keys are the same), so `resolved_imports` and `module_graph` stay green.
- `check_scc` reruns for the edited file's unit; its `exports` are compared to the old value. Unchanged exports mean `decl_shape` projections stay green and importers' `check_scc` do not rerun.
- A unit with `incomplete: true` (budget exceeded, cancelled, crashed) is reported but its exports are not trusted: mark the output with a flag that forces dependents to treat missing declarations as `ERROR` and, if you persist anything, never persist it.

### 6.4 Module resolution and the file system

Salsa tracks only what queries read through the database. The resolver reads the file system directly, so its answers are invisible.

Design:
- `Project.resolution_epoch` is an input incremented whenever a file is added, removed, or renamed, or when a `package.json`, `tsconfig` or symlink target the resolver may have consulted changes (the watcher decides conservatively).
- `resolved_imports` reads the epoch, so all resolutions are recomputed when it moves. This is coarse but correct; refine later by tracking which directories the resolver touched.
- Resolver results are cached in L3 keyed by (importing directory, specifier, conditions); clear affected entries when the epoch moves.
- Library files and `node_modules` can be given high durability [VERIFY wording in the pinned Salsa], so verification of unchanged revisions skips them quickly.

### 6.5 Invalidation matrix (acceptance tests for stage 4)

| Scenario | Expected recomputation |
|---|---|
| Unchanged save (same bytes) | none; no new revision |
| Whitespace or comment edit | `module_header` and `check_scc` of that file's unit rerun; outputs equal, nothing downstream reruns |
| Function body change, same signature | that unit reruns; exports equal; importers green |
| Exported signature change | that unit reruns; changed `decl_shape` projections; only importers that read that declaration rerun |
| Unrelated export of the same file unchanged, another changed | importers that read only the unchanged declaration stay green |
| Import added or removed | `module_header`, `resolved_imports`, `module_graph`, affected units |
| Dependency deep in the chain changes shape | rerun along the dependency path until outputs become equal |
| Change inside a cycle | whole SCC unit reruns; other units depend only on its exports |
| File added, removed or renamed | epoch moves; resolutions and graph recompute; units whose resolved imports changed rerun |
| `incomplete` unit | never cached as authoritative; reruns next revision |

Each row becomes an integration test asserting *which queries executed* (use Salsa's event logging or counters), plus an equality check against a cold full run.

### 6.6 Petgraph versus Salsa responsibilities

| Concern | Owner |
|---|---|
| "Must this be recomputed?" | Salsa |
| Order of execution, cycles between files | petgraph and the scheduler |
| Dependency edges between declarations | Salsa (recorded by `decl_shape` reads) |
| `rdeps` for display or tooling | petgraph |

Do **not** maintain a second invalidation mechanism by walking `rdeps` to mark things dirty; that duplicates Salsa and will disagree with it.

### 6.7 Interaction with the scheduler [VERIFY]

Two possible shapes; choose after S1:
1. **External scheduler, memoized queries:** the scheduler walks ready SCCs in parallel, each worker calling `check_scc` with its own database handle. Dependencies are already computed, so inner reads hit memoized results.
2. **Salsa-driven recursion:** let `check_scc` pull dependencies on demand and use the library's parallelism helpers. Simpler, but parallel behavior depends on the pinned version.

Either way, writes to inputs while workers run must cancel in-flight work; use the library's cancellation mechanism and treat a cancelled unit as no result.

---

## 7. Checker integration and migration

### 7.1 What stays

`subtyping.rs` algorithms (union, function, object merge-join, bivariant methods, coinduction), `semantic/generics.rs` inference and substitution, narrowing, expression and statement checking, `SemanticQueries` as the seam, the diagnostic catalog, `line_index`, and `type_display` (adapted).

### 7.2 What changes, file by file

| Current | Change |
|---|---|
| `arena.rs` | Replace with the arena in section 1: immutable nodes, `DeclSlot`, no `set`, no placeholders, no `display_names`/`record_value_types`; keep digest interning and verification |
| `types.rs` | `Type` becomes `Node`; `ObjectType` properties become bump slices; `TypeParameterId` becomes `TypeParamKey` |
| `namespace.rs` | Flat name table becomes per-module scope tables; entries produce `DeclSlot`s; instantiation memo becomes `inst_cache`; deferred diagnostics stay but live in the unit context |
| `symbol_map.rs` | Unchanged idea (dense vector by symbol index) but per file; imports map to `DeclSlot`s |
| `bridge/context.rs` | `CheckContext` becomes the unit context holding the arena, caches and the provider |
| `bridge/mod.rs` | `check_program` becomes `check_unit` (phases A, B, C); parse inside the worker |
| `subtyping.rs` | Add `expand` calls before structural comparison; handle `Ref`, `Named`, `App` |
| `semantic/queries.rs` | Unchanged API; relation cache lives in the arena |
| `lib.rs` | `TypeChecker::check_source` stays for single-file use; add a project API |

### 7.3 Library and ambient declarations

- Built-in library declarations (`Array`, `Promise`, `Record`, DOM, and so on) are a read-only set of portable declarations loaded once at L3 and materialized lazily per arena through the same `DeclProvider` path.
- Global augmentation and declaration merging across files: merge by `DeclKey` (same name, same merge group). Define merge order by file order so results are deterministic. This is a known hard area (HLD risk 4); write fixtures before implementing.

### 7.4 Migration order (each step builds and passes the existing fixtures)

1. Introduce `Node` and `DeclSlot` alongside the current types behind the same `TypeId` API; port interfaces and classes to `Ref` slots; delete placeholders and `set`. **Done.**
2. Move display and aliases to `Named` and `App`; delete `display_names`, `duplicate_named`, `make_unique`. **Done.** `App` keeps its instantiated body, so expansion is still eager; lazy `expand` comes with step 4's unit context.
3. Switch object properties to bump slices with `SmallVec` scratch; add the `needs_drop` assertions.
4. Add `Worker` and `run_unit`; check one file per unit with reset between runs.
5. Add keys, stable hashing, portable export and import; round-trip tests.
6. Add the module graph and sequential multi-file driver.
7. Add the scheduler, then the Salsa layer.

---

## 8. Determinism and limits (ADR-6)

### 8.1 Ordering rules

| Output | Order |
|---|---|
| Files | canonical path, byte-wise |
| Diagnostics | (file index, start, code, message) |
| Export tables | source order of declarations, as an ordered map |
| Merged declarations | file order, then source order |
| Union display | as written (first writer) |
| Anything hashed | explicit canonical encoding (3.2) |

### 8.2 Forbidden in code that influences output

- Iterating `std::collections::HashMap`/`HashSet` or `FxHashMap` to produce output (use ordered maps or sort first).
- Randomly seeded hashers.
- Using `Name` numbers, `TypeId` numbers, thread ids, timestamps or completion order for ordering.

Enforce with `clippy.toml` `disallowed-types`/`disallowed-methods` for the output-facing modules, plus the determinism test.

### 8.3 Constants to calibrate with data

`max_nodes_per_unit`, `max_bump_bytes`, `max_instantiation_depth`, channel capacity, and the threshold at which a worker's bump is dropped and recreated after an unusually large unit. Choose from harness measurements, not guesses.

---

## 9. Tests and measurements

### 9.1 Unit and property tests

Arena (1.12), identity (3.7), union and object canonicalization, `expand` termination, reorder buffer (out-of-order delivery yields in-order release), scheduler (random DAGs and cycles: every unit runs exactly once, after its dependencies).

### 9.2 Integration tests

- Cross-file fixtures: imports of variables, functions, classes, interfaces, aliases, generics, re-exports, `export *`, default and namespace exports, type-only imports, circular imports.
- The invalidation matrix (6.5), each asserting both the executed-query set and equality with a cold run.
- Merge and ambient declaration fixtures (7.3).

### 9.3 Determinism test

Run the same project many times with 1, 2 and N threads and injected random delays at stage boundaries; require byte-identical diagnostics and exports. Run the same run with different hasher seeds for any `std` maps (to catch accidental iteration-order leaks).

### 9.4 Safety checks

- Run the arena and worker tests under Miri where feasible and under AddressSanitizer for the bump-reset paths [VERIFY availability for the toolchain in use].
- Compile-fail tests: a reference to arena data surviving `run_unit` must not compile.

### 9.5 Measurements per stage (for decisions, not gates)

| Metric | Why |
|---|---|
| Time and peak memory, cold and warm | baseline and regressions |
| Nodes, bump bytes, relation-cache size per unit | budget calibration |
| Materialization time and count (rebuilt declarations) | decides ADR-1 revisit (S5) |
| Largest SCC and its share of wall time | decides ADR-3 revisit (S4) |
| Query executions, cache hits per scenario | validates cutoff |
| Allocation counts (dhat) and bump bytes separately | allocator comparisons |

---

## 10. Risks and open questions

| # | Risk or question | Plan |
|---|---|---|
| 1 | Salsa version behavior (backdating, handles, cancellation, durability naming) | S1 spike; pin; keep a thin adapter module |
| 2 | Unannotated exports couple shapes to bodies and may delay downstream units | Phased unit (4.3), annotated fast path; split publication later |
| 3 | Typed handles: how strictly to prevent `TypeId` escaping the arena | Decide in stage 1: phantom lifetime on `TypeId` or lint |
| 4 | Declaration merging and `declare global` across files | Fixtures first; merge by `DeclKey` with deterministic order |
| 5 | Resolver invisibility (symlinks, package exports, path mappings, project references) | Conservative epoch (6.4), refine with touched-directory tracking |
| 6 | Re-parsing cost (header parse plus check parse) | Measure; consider keeping the parse on the checking thread |
| 7 | Isolation duplicates imported-type work | Lazy rebuild plus memo; shared read-only library set if needed |
| 8 | Hash choice if source is adversarial | Feature flag for truncated cryptographic hash |
| 9 | WASM: threads and allocator | Same scheduler interface with a single-thread implementation; keep separate build profile |
| 10 | Runtime and typed IR (value representation, cycles, GC) | Out of scope here; decide before any backend work |

**Decisions (locked, HLD section 7):** identity/version split; alias wrapper nodes; union display first-writer-wins; SCC as the v1 unit; XXH3-128; lazy stable hashing; Salsa `=0.28.5` provisional until S1; the module key rule (3.1.1); `FileId` as a run-local index sorted by path.
