# Generics

> **Status: substantially complete.** Type parameters are supported on functions, interfaces, type aliases, and classes; inference, substitution, defaults, explicit type arguments, constraints, and recursive generic shapes all work. This replaces the earlier `generics-tier1.md`, which described a functions-only implementation that no longer reflects the code. What's genuinely still missing is listed at the end, not scattered through the document as caveats.

This document follows generics as it actually grew: one new `Type` variant, a structural inference algorithm, a substitution pass — and then, notably, two capabilities (generic classes, recursive types) that needed almost no new code of their own once the first three pieces existed in the right shape. That last part is the most interesting design decision in this file. The part of it that lives in the arena, how a declaration gets an id before its members exist, is told in section 2.5 of [arena](arena.md); this document is the semantic story, that one is the implementation story.

## 1. One new type variant, not a parallel type system

```rust
GenericParameter(TypeParameterId, String, Option<TypeId>),
```

A type parameter is represented as an ordinary `Type` variant, the same enum that already holds `Number`, `Array`, `Object`, and everything else. Nothing downstream (subtyping, the arena, diagnostics) needs a special case for "is this a generic." A `GenericParameter` is just a `TypeId` like any other until something specifically pattern-matches on it during inference or substitution.

The `String` is the parameter's source name (`"T"`), kept for pattern-matching readability and diagnostics — it is never used as an equality key. The `Option<TypeId>` is the parameter's own `extends` bound, resolved once when the `GenericParameter` is first created: `None` means fully unconstrained.

## 2. Identity: `TypeParameterId`, not a name or a pointer

A type parameter's identity is derived from where it is written in the source, not from an arena slot and not from an AST pointer:

```rust
pub struct TypeParameterId {
    file_id: FileId,
    declaration_span_start: u32,
    parameter_index: u32,
}
```

The `file_id` was added when sessions and file identity landed: two files' parameters at the same byte offset would otherwise be the same id once they share semantic state (see section 4 of [case-study](../case-study.md)). Two type parameters — in the same declaration or different ones — can only share a `TypeParameterId` if they are, in fact, the same declared parameter. This is what lets a function's own `T`, an interface's own `T`, and a class's own `T` all be spelled the same without ever being confused with each other: `Box<T>`'s `T` and `identity<T>`'s `T` get different `declaration_span_start`s, so they're different `GenericParameter` nodes even though nothing about their name distinguishes them.

This replaced an earlier version keyed on the `TSTypeParameter` AST node's raw pointer address. The pointer version worked for the same reason — a stable key across the declare pass and the check pass, which both independently need to agree on the same node for `T` — but tied identity to one process's memory layout rather than to a property of the source text itself. A source-derived key is stable across separate parses of unchanged source; a pointer is only guaranteed stable within the one parse that produced it.

## 3. Scoping: shadow a name in the flat namespace, not a scope tree

`TypeNamespace` is a single flat map from name to declaration — there's no block or module scoping for types anywhere in this checker (see [architecture.md](../architecture.md)). Generic type parameters fit into that same flat model by *temporarily shadowing* a name:

```text
enter function<T>(...)
       -> push_type_params
              -> TypeParameterId::new(T's own span start, index 0)
              -> namespace["T"] = GenericParameter(id, "T", constraint) at a cached TypeId
       -> resolve params/return type/body against that namespace
       -> pop_type_params
              -> namespace["T"] restored to whatever it pointed to before
```

`push_type_params` is the function-shaped entry point; `push_decl_type_params` is the declaration-shaped generalization it's built on, since an interface's or a type alias's `<T, ...>` is a `TSTypeParameterDeclaration`, not an oxc `Function`. A class's `<T, ...>` is the same shape too — which is the first half of why generic classes needed almost no class-specific scoping code (see §7).

The cache backing this is keyed on `TypeParameterId`, so a generic function's signature (resolved once, up front) and its body (resolved later, in a separate pass) agree on the exact same `GenericParameter` node for `T` — without this, a `T[]` annotation inside the body would never structurally match the parameter `T` came from.

## 4. Inference is structural, not declarative

At a call site, nothing is told which argument corresponds to which type parameter. `infer_type_param_bindings` walks the declared parameter type and the actual argument type in lockstep, recording a binding the first time it reaches a `GenericParameter` leaf, keyed on `TypeParameterId`:

```text
declared param type          argument type
Array<T>              <->    Array<number>
   |                            |
   v                            v
 T (id)                        number
       binding: id -> number
```

The walk recurses through arrays, object properties, and function parameter/return positions. A type parameter that never appears in a reachable position is left unbound, and substitution falls back to `unknown` for it rather than `any` — a failed inference should never silently suppress further checking at the call site.

**Multiple candidates resolve through subtyping, not unioning.** A second argument resolving to an already-bound parameter doesn't just keep the first binding and reject the rest, and it doesn't union the two candidates either — checked against real TypeScript first: `pair<T>(a: T, b: T)` called as `pair(1, "x")` is a genuine TypeScript error, not an inferred `T = number | string`. Instead each new candidate is resolved against the existing binding through ordinary subtyping: if one is a supertype of the other, the binding widens to it (`pick(dog, animal)` infers `T = Animal`); if neither is, the binding is left alone and the real mismatch still surfaces through the ordinary per-argument assignability check.

**Literals are widened unless the bound asks for them.** A bare `identity(5)` binds `T` to `number`, because a generic call is expected to give the widened type. A bound that is, or contains, a primitive or a literal (`T extends "a" | "b"`, `T extends number`) changes that: the bound is asking for the narrow type, and widening would bind `T` to `string` and then reject its own argument. TypeScript keeps the literal in exactly this case, so inference does too. Two literals of the same primitive under such a bound combine (`pair(1, 2)` gives `T = 1 | 2`); literals of different primitives do not, and the first binding stays so the second argument is reported by the ordinary assignability check. A bound with no primitive in it (`T extends { n: number }`) still widens.

**Explicit type arguments are locked, not just another inference input.** `identity<string>(x)` matches its type arguments positionally against the type parameters in *declaration order* (recovered separately, since a resolved `FunctionType` itself doesn't remember declaration order) and locks them — inference can't override an explicitly given argument, only fill in ones left unspecified. A type argument that cannot be resolved becomes the error type in its position instead of being dropped. The arguments are matched to parameters by position, so dropping one would shift every later argument onto the wrong parameter; the error type is compatible with everything, so the parameter it lands on stops constraining its arguments without causing a second diagnostic.

## 5. Substitution rebuilds the shape, never the declaration

Once bindings are known, the relevant type is rebuilt with every `GenericParameter` replaced, matched by `TypeParameterId`:

```text
return type: T (id)
bindings: [(id, number)]
      -> substituted: number
```

`substitute_type_params` walks the same shapes inference reads and allocates a fresh, substituted copy. The original declaration — a function's stored signature, an interface's cached generic shape, a class's cached instance shape — is never touched, so the same generic declaration can be used with different type arguments at every call site or reference without drifting. `contains_type_param` short-circuits this for the ordinary non-generic case: a type with no `GenericParameter` anywhere returns unchanged, no walk, no reallocation.

## 6. Constraints (`T extends X`) and defaults (`T = X`)

A type parameter's `extends` bound is resolved once (see §1) and enforced everywhere that parameter is used: at a call site, against whatever `T` is inferred as; at a reference with an explicit type argument, against what was given. Both go through the same structural assignability check every other type relationship in this checker uses — there's no separate "constraint satisfaction" algorithm.

A constrained parameter's own members are usable inside the declaration's body: `value.length` type-checks against `{ length: number }` for `T extends { length: number }`, since the constraint is the one guarantee the checker actually has about what `T` could be. An unconstrained parameter still has no members at all.

A default (`<T, U = T>`) is resolved with the declaration's own earlier parameters already in scope, so a later default can refer back to one (`U = T`) — `resolve_type_param_default` pushes the same declaration-shaped scope described in §3 before resolving the default expression. The result can still contain `GenericParameter` placeholders from an earlier parameter; the caller substitutes whatever it already has for those when applying the default.

## 7. Generic interfaces, aliases, and classes: the same mechanism, three declaration kinds

Generic interfaces (`interface Box<T> { value: T }`) and type aliases (`type Pair<A, B> = { first: A; second: B }`) resolve through exactly the pipeline described in §3–§5: their own `<T, ...>` is pushed as a shadow before their body resolves, the resulting shape is cached with bare `GenericParameter` placeholders still in it, and each reference (`Box<number>`, `Pair<string, boolean>`) substitutes against that one cached generic shape independently — two instantiations of the same generic type stay distinct (`Box<number>` is never confused with `Box<string>`) because substitution allocates a fresh copy per reference rather than mutating the cached shape in place.

**Generic classes needed almost no class-specific code**, because `resolve_class` (the function that walks a class's fields and methods into a structural shape) has no idea classes can be generic at all — it just calls the same `resolve_type_annotation`/`resolve_function_params` helpers every other declaration kind uses, which look names up in the shared namespace. Since the *caller* (`resolve()` in `namespace.rs`) already pushes the class's own `<T, ...>` as a shadow before invoking `resolve_class`, a field typed `value: T` or a method parameter typed `T` resolves correctly with zero changes to `resolve_class` itself. The one place this composition initially *didn't* reach was checking constructor and method bodies, which is a separate pass from declaration resolution; section 12 of [case-study](../case-study.md) tells that story.

**Type argument count is checked the same way for every generic kind.** `Box<number, string>` for a single-parameter `Box<T>`, or a bare `Box` reference where `Box<T>` has no default, both go through the same arity check (`declared_type_param_arity`/`declared_type_param_decl` in `namespace.rs`), which reports the required-vs-total parameter counts once per declaration regardless of whether it's an interface, an alias, or a class.

## 8. Recursive types: interfaces, classes, and (limited) generics together

A self-referencing shape — `interface Node { next: Node | null }`, a linked list; `class Box<T> { next: Box<T> | null }`, a recursive generic class — resolves rather than being rejected as circular. The mechanism is shared by interfaces, classes, and object-literal aliases alike: the declaration is a `Ref` to a slot that is handed out *before* its members resolve, and `resolve_ref` gives the slot its body once they finish. The `Ref` itself is never rewritten. An earlier version allocated an empty placeholder object and overwrote it in place; that was replaced on 2026-10-05 (see sections 2.5 and 4 of [arena](arena.md)). It is covered there because it is really an arena and `namespace.rs` design decision more than a generics-specific one.

What's still limited here: this protects *declaration*-level recursion (a type referring to itself by name while resolving). It is not a general coinductive relation checker — two recursive types being compared for subtyping still walk their full structure each time rather than detecting "I've compared these two nodes before" during the comparison itself, which is fine for the depths that show up in realistic code but would eventually need its own guard for a pathological deeply-nested case.

## 9. Builtin generics: `Array<T>` fully modeled, `Promise<T>` deliberately opaque

`Array<T>`/`ReadonlyArray<T>` are modeled with real element-type substitution — indexing, destructuring, and `for...of` all propagate the element type correctly, including `| undefined` under `noUncheckedIndexedAccess`-equivalent settings (see the `array_indexing_*` and `array_destructuring_*` fixtures).

`Promise<T>` is a deliberate exception: it resolves to an opaque generic type with *no* modeled members at all — no `.then()`, no `.catch()`, nothing — rather than a partial, likely-wrong model of the real `Promise` interface. A fixture (`tests/fixtures/builtin-generics/promise_member_access_correctly_reports_missing_property.ts`) documents this explicitly as an accepted, intentional divergence from real `tsc` (which does have `.then()`): modeling `Promise` properly needs `async`/`await` and `Awaited<T>` support that doesn't exist yet, and a half-modeled `Promise` that's right about the type parameter but wrong about every method would be a worse failure mode than an honestly opaque one. See `docs/purpose-and-overdesign.md` for the general philosophy this follows.

## 10. What's still genuinely missing

- **Contextual inference is still missing, and it is the largest gap for everyday code.** Callbacks passed to generic functions (`map`, `reduce`) do not get their parameter types from the call, and callback parameters are inferred covariantly.
- **Inference walks arrays, objects, functions and unions only.** Tuples and `Promise<T>` contribute no bindings.
- **`const` type parameters** and **variance modifiers (`in`/`out`)** on declared type parameters.
- **Full coinductive subtyping** for recursive types compared against each other (see §8).
- **`Awaited<T>` and real `Promise<T>` modeling** (see §9). `async` and `await` are checked now, so what blocks this is real library shapes through the provider; see [library-and-ambient-declarations](library-and-ambient-declarations.md).

For what the checker catches across the whole type system, measured against `tsc` and not just for generics, see [language-coverage](../language-coverage.md).
