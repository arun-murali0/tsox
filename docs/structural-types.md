# Structural Types: Declarations and Resolution

> **Architecture case study:** how tsox moved from primitive checking to a reusable structural type system.

## 1. What changed from Foundation

Structural Types introduced object types, properties, structural subtyping, optional properties, arrays, unions, aliases, interfaces, forward references, circular references, and function arity.

Fixtures cover: aliases, arrays, excess properties, arity, interfaces, nested objects, optional properties, unions, unresolvable annotations, and width subtyping.

## 2. Structural typing as the core model

TypeScript's ordinary object compatibility is structural. `crates/tsox-types/src/types.rs` therefore models object shape directly:

```rust
pub struct ObjectType { pub properties: Vec<PropertyEntry> }
pub struct PropertyEntry { pub name: Rc<str>, pub type_id: TypeId, pub optional: bool }
```

A property carries its name, type, and optionality because all three affect assignability.

`name` is `Rc<str>` rather than `String` because class-inheritance resolution
clones the whole accumulated property list once per level of an inheritance
chain (see `resolve_class` in `namespace.rs`), which puts `PropertyEntry`
cloning on the hot path for any class hierarchy of meaningful depth. An
`Rc<str>` clone is a refcount bump; a `String` clone is a heap allocation and
byte copy every time. It's the kind of detail that looks like premature
optimization in isolation, and only makes sense once you know which caller is
actually paying for it repeatedly.

## 3. Width subtyping

`crates/tsox-types/src/subtyping.rs` checks that every required target property exists in the source and that matching property types are compatible. Extra source properties are acceptable for ordinary structural assignment.

This is protected by `width_subtyping_extra_prop_ok.ts`, `width_subtyping_missing_prop_error.ts`, `width_subtyping_multiple_props.ts`, and `nested_object_subtyping.ts`.

## 4. Optional property semantics

The `optional` bit is semantic, not decorative. A source optional property cannot satisfy a required target property. This rule is explicitly covered by `optional_property_cannot_satisfy_required.ts`.

This is a good example of why a real semantic object model was needed instead of a plain name/type map.

## 5. Excess-property checking on fresh object literals

TypeScript's width subtyping (§3) is deliberately permissive: an object with
extra properties is still assignable wherever the required shape is present.
But TypeScript also rejects this:

```ts
interface Point { x: number; y: number }
const p: Point = { x: 1, y: 2, z: 3 }; // error: 'z' does not exist on Point
```

which looks like a contradiction of §3 until you notice the difference isn't
the *shape*, it's *where the object literal came from*. Width subtyping is
about values in general; excess-property checking is a narrower rule that
applies only to an object literal written directly at the point it's checked
against a target type — what TypeScript calls a "fresh" literal.

```ts
const raw = { x: 1, y: 2, z: 3 };
const p: Point = raw; // fine -- raw is a variable, not a fresh literal
```

The same value, once it's passed through a variable, stops being fresh and the
extra property is allowed again, exactly as real `tsc` behaves. `crates/tsox-checker/src/bridge/
expressions/excess.rs` implements this as its own check, run only where a
literal is genuinely fresh — directly in a variable initializer, a return
statement, or an argument position — rather than folding it into
`object_is_subtype` in `subtyping.rs`, since ordinary structural subtyping and
excess-property checking are answering two different questions (`is one type
usable where another is expected?` vs. `did the author of this literal
probably make a typo?`) and conflating them would make width subtyping wrong
for every non-literal case.

This is covered by `excess_property_literal.ts`, `nested_excess_property_
literal.ts`, `excess_property_in_return_and_argument.ts`, and `extra_property_
on_a_non_fresh_object_is_allowed.ts`, which exists specifically to protect the
"stops being fresh once assigned to a variable" rule from regressing.

## 6. Arrays and unions

Arrays use `Type::Array(TypeId)`, so element types are ordinary arena types. Unions use `Type::Union(Vec<TypeId>)` and are normalized by `TypeArena::alloc_union`, which flattens nested unions, removes `Never`, collapses one-member unions, and removes duplicates.

The Structural Types covariance fixtures intentionally record the current compatibility/soundness boundary.

## 7. Declaration namespace

`TypeNamespace` maps declaration names to aliases, interfaces, classes, or resolved types. Declaration lookup is separated from expression checking.

```text
name -> TypeEntry -> alias/interface/class/resolved TypeId
```

This keeps name resolution out of every type operation.

## 8. Lazy resolution and recursion protection

A declaration entry has `resolved` and `resolving` state. Resolution therefore behaves like a small state machine:

```text
unresolved -> resolving -> resolved
                 |
                 +-> circular reference detected
```

Lazy resolution permits forward references and recursive declarations without requiring declaration order to match dependency order.

## 9. Annotation resolution boundary

`crates/tsox-types/src/type_annotation.rs` translates Oxc `TSType` nodes into tsox `TypeId` values. AST details stop at this boundary; the semantic core consumes `TypeId`.

## 10. Function compatibility

`FunctionType` and `Param` were introduced as semantic callable representations. Parameter metadata already records type, optionality, and rest-ness, giving later stages a stable place to implement richer call behavior.

## 11. Why these patterns were chosen

The stage deliberately uses composition:

- object subtyping composes property entries
- arrays compose `TypeId`
- unions compose existing types
- declarations resolve to semantic types
- functions reuse parameter/type representations

The goal is to add language features by extending reusable semantic primitives, not by adding syntax-specific special cases.

## 12. What Structural Types established

- structural object types
- width subtyping
- optional-property semantics
- excess-property checking on fresh object literals
- arrays
- unions and normalization
- aliases/interfaces/classes as declarations
- lazy resolution
- forward/circular reference handling
- function arity
- reusable annotation resolution

The main architecture lesson is: **Oxc owns front-end infrastructure; tsox owns stable semantic types and relationships.**
