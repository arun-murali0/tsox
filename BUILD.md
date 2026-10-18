# tsox - Build Stages (Real-World-First Order)

Derived from checklist.md. Reordered to prioritize the type-system features and
real-world code patterns most commonly hit in everyday TypeScript, ahead of
theoretically "foundational" but less-frequently-used advanced type operators.

Module resolution uses `oxc_resolver` for path/specifier resolution (node_modules
walk, package.json exports/main/types, tsconfig paths/baseUrl, extension probing).
Everything checker-side (symbol tables, cross-file lookup, .d.ts parsing,
import-cycle handling) is tsox's own work. Cross-file lookup and cycle handling are
done for annotated and inferred exports (docs/architecture/cross-file-types.md); .d.ts
parsing is not.

---

## Engine track - finish this before the advanced stages

Problem: the advanced stages (3 to 9) keep adding type system features on top of an engine
that is only partly finished. A cross-file gap, a crash on a big file, or a result that
depends on thread timing would then show up inside a feature that did nothing wrong. The
decision is to finish the stable engine first and start advanced types after it.

The plan follows the LLD sections (5, 6, 7, 8, 9), and each stage has an exit test so it is
clear when it is done. Scope for now is E0 to E6. E7 and E8 follow; E8 also runs next to
every other stage.

E0 - Safety net. A harness that runs every corpus file alone and records aborts, and the
determinism test (1, 2 and N threads, injected delays, byte identical output).
tests/typescript-testcases (10,083 files, known-crashers-6.0.3) and scripts/compare-local.sh
already cover part of this. Exit: zero aborts on the corpus outside the known crashers, and
identical output across thread counts.

E1 - Finish cross-file completeness. Make the owner key of a nested generic signature
position free (today it is built from the source offset of the first type parameter, so a
line added above a declaration changes its hash); build it from the parent declaration key
and a path inside it instead, with a test that a blank line above changes no hash. Export overload sets (today only the first overload
crosses and the unit is partial), `export * as ns`, default expressions, anonymous default
function and class. [need to check: carry an overload set as an intersection of functions]
Exit: on a real project the partial units are near zero and each remaining one has a named
reason.

E2 - Graph shape and edge kinds (LLD 4.1, 4.2). Flat graph vectors (scc_of, sccs, topo), a
stable key per cycle from its sorted member keys, and edge kinds as flags (value, type only,
side effect, dynamic, re-export). A dynamic import should only order files when its type is
used; today every dynamic import with a string is an ordinary edge and can invent a cycle.
Exit: unit tests on random graphs for order and membership.

E3 - A cycle as one unit (LLD 4.3, 4.4). Cycle members are checked again for up to four
rounds today. The HLD locks one cycle as one unit with phases declare, shapes, bodies, one
arena, shared slots, and a circularity error when an inferred type re-enters itself.
Exit: the cycle tests pass without the round limit and the output is stable.
[yet to decide: build the unit as locked, or keep the iteration and amend the HLD]

E4 - Scheduler and pipeline (LLD 5). Dependency counting per unit, a ready queue, ordered
publication, a read and hash and header stage with backpressure, one internal error
diagnostic when a unit panics (its dependents still run), cancellation checked at unit start.
Exit: every unit runs exactly once after its dependencies on random graphs, and E0 stays green.

E5 - Project config and resolution (LLD 6.4). tsconfig: extends, paths, baseUrl, rootDirs,
typeRoots, types, include, exclude and the strict flags; package.json exports and types
conditions; a resolution epoch as a project input. Then the gate on missing package imports
comes off. Exit: a project that uses `paths` and package exports checks with no false TS2307.

E6 - Library and ambient declarations (LLD 7.3). A library loading path through the same
provider as dependencies, with a small hand written subset (Array, Promise, Record) until the
advanced stages, because the real library files need mapped and conditional types. Global
declarations, `declare global` and module augmentation, fixtures first, merged by key in file
order and then source order. Typed `import()` and `typeof import()`, which need Promise.
Exit: merge fixtures pass and give the same result in any file order.

E7 - Query layer (LLD 6). Spike S1 first: pin the Salsa version and check early cutoff,
thread handles and cancellation. [need to check: S1 outcome] Then the file guard, inputs,
projection queries and the recorded reads. Exit: every row of the invalidation matrix in LLD
6.5 is a test that asserts which queries ran and compares with a cold run.

E8 - Limits, safety, measurement (LLD 2.5, 8, 9). Budgets and an instantiation depth limit
with a TS2589 style error, one shared guard for anything that can re-enter itself, Miri or
ASan on the reset paths, a compile-fail test that arena data cannot outlive a unit, an audit
for hash map iteration in output code, and the metrics from LLD 9.5.

Engine exit gate: E0 to E8 exit tests green, no abort on the corpus, byte identical output
across thread counts, a body only edit rechecks one unit, and baseline numbers recorded.
Deferred to the advanced stages: everything in Stages 3 to 9, tuples, bigint, symbol, const
assertions, template literal types, abstract and static class sides, async and generators,
and the real library file content.

---

## Stage 1 - Generics completion

Section 6, remaining items (most of §6 is already `[x]`). Shipped since: inference
that keeps literals under a primitive or literal bound, explicit type arguments that
stay in position when one cannot be resolved, and a fingerprint-keyed instantiation
memo.
- Inference from return positions (full)
- Inference priority / multiple candidates (full algorithm)
- Contextual inference into generic calls
- Higher-order function inference
- Constraint checking under substitution (full)
- Generic type parameters used as type arguments to other generics
- `const` type parameters
- Variance modifiers (`in`/`out`)

Why first: highest real-world density. Everyday generic code (`map`, `reduce`,
curried functions, factory functions) routinely hits inference edge cases that
are currently unhandled. No new `Type` variant needed - this deepens the
existing substitution/inference code in `semantic/generics.rs`.

Dependencies: none. Extends existing code.

---

## Stage 2 - Module resolution (foundation)

Section 11, core subset:
- ES module imports/exports (types + values)
- Re-exports, default exports
- `import type` / `export type`
- Cross-file symbol lookup

Path/specifier resolution delegated to `oxc_resolver`. Remaining work is
checker-side:
1. Module graph/driver - DONE, behind the `module-resolution` feature (see
   docs/architecture/module-resolution.md): `ModuleGraph` resolves each import via
   `oxc_resolver`, keeps unresolved edges, finds cycles, orders files into
   dependency layers, and `check_project` checks the layers in parallel.
2. Each checked file exposes an export table, and import sites look into the
   file they name - DONE (`UnitOutput.exports` and `export_names`, bound by
   `bridge/imports.rs`; see docs/architecture/cross-file-types.md). Cycle
   members are checked again until their exports settle, and a missing name or
   module is reported (TS2305, TS2307, TS1192) only against a complete export list.
3. `import type` as a checker-side distinction - the type-only flag is read when
   an import is bound.
4. Classes, enums, namespaces, `export *`, `export =`, default classes and default
   identifiers cross files; a package's `.d.ts` is read for what it exports. Still to do:
   export overloads, `export * as ns`, default expressions and anonymous defaults (E1).

Why here: unblocks every other stage from being single-file-only, which is
the single biggest realism gap in real-world usage - almost no real
TypeScript project is one file.

Not done: tsconfig `paths`/`baseUrl` (the resolver is built without a tsconfig, so a missing
package import is only reported when nothing could be hiding it), declaration merging and
augmentation across files, and re-checking only changed files and their dependents (E5, E6,
E7 in the engine track).

Dependencies: none structurally, but benefits from Stage 1 being stable so
generic types resolve correctly once they can cross file boundaries.

---

## Stage 3 - Tuples & intersections

Section 1.4:
- Tuple types, readonly tuples, optional/rest elements - not done
- Intersection types, intersection reduction - DONE (docs/architecture/intersections.md)
- Union reduction / subsumption

Why here: extremely common in real code (function signatures with fixed-shape
arrays, `type A = B & C` patterns, discriminated union members). Builds on
existing `Array` type and subtyping/cycle-guard code; no dependency on
Stages 1-2, but ordered after them since generics and modules touch more of
the codebase's core flow.

Dependencies: none.

---

## Stage 4 - Narrowing upgrades

Section 9. Shipped: `instanceof`, `in`, discriminated unions (a property of an
identifier, in `if` and `switch`), `switch` grouped labels / default / `typeof`,
`&&` / `||` conditions, assignment narrowing, branch joins and loop exit
narrowing. See docs/architecture/narrowing.md.

Remaining:
- Switch exhaustiveness
- User-defined type predicates
- Assertion functions (`asserts x is T`)
- Property-path narrowing (`a.b.kind`, `a.b !== null`), which needs the narrowing
  key to become a path instead of a symbol
- Equality between two variables, optional-chain narrowing, `switch (true)`
- A join after `switch`, and narrowing of captured variables inside closures

Why here: the shipped forms cover the everyday patterns (error handling, API
response shapes, class hierarchies). The remaining items each need something new
(a path-shaped key, a predicate type on function types, an exhaustiveness check
against `never`) so they are separate pieces of work, not more handlers.

Dependencies: property paths and predicates are independent of Stage 3.

---

## Stage 5 - `keyof` / indexed access

Section 8.1 (partial):
- `keyof T`
- `T[K]`

Why here: common in real code (`Pick`-style helper types, generic property
accessors), but less frequently hand-written directly than tuples/narrowing.
Hard prerequisite for Stage 6.

Dependencies: Stage 3 (tuples/intersections need keys/indices to describe).

---

## Stage 6 - Mapped & conditional types

Section 8.2, remainder of 8.1:
- `T extends U ? X : Y`, distributive conditionals, `infer`
- Homomorphic / non-homomorphic mapped types
- Key remapping (`as`), `+readonly`/`-readonly`/`+?`/`-?`

Why here: powers most of the standard library (Stage 7) but is less
frequently hand-authored directly in application code compared to
Stages 1-5. Heaviest recursion; will need to generalize the existing
cycle-guard pattern from `subtyping.rs`/`semantic/generics.rs`.

Dependencies: Stage 5.

---

## Stage 7 - Standard library utility types

Section 14:
- `Pick`, `Omit`, `Partial`, `Required`, `Record`, `Readonly`
- `ReturnType`, `Parameters`, `InstanceType`, `Awaited`
- `NonNullable`, `Extract`, `Exclude`

Why here: extremely common in real code, but mechanically just named
mapped/conditional types - cheap once Stage 6 exists, one-off hacks if
attempted earlier.

Dependencies: Stage 6.

---

## Stage 8 - Overload resolution

Section 3.3, 3.4, 4, 5:
- Multiple call/construct signatures
- Method overloads, overload declarations, implementation checking, resolution
- Generic call signatures

Why here: real-world-common (especially in library-style code and DOM-like
APIs) but touches the most call sites (functions, methods, constructors) -
placed after the type-shape stages so overload candidates compare against a
richer type system.

Dependencies: none required; ordered after Stage 6 for practicality.

---

## Stage 9 - Contextual typing

Section 10:
- Contextual typing of parameters, return, object/array literal
- Best common type / candidate unions
- Generic contextual inference

Why last: real-world-important (this is what makes callback parameter types
"just work" without annotations) but the largest cross-cutting pipeline
change - requires threading an expected type through expression inference
everywhere, not a localized addition. Already flagged as unimplemented in
existing code comments.

Dependencies: none required; ordered last due to size/risk.

---

## Set aside - full module ecosystem features

Beyond the Stage 2 foundation. Done since: ambient modules (`declare module "x"`), namespaces
with merging inside one file, `.d.ts` for packages and the `@types` fallback. Still set aside:
- Wildcard ambient modules
- Module/global augmentation and declaration merging across files (E6)
- UMD / global scripts vs modules

Why separate: `oxc_resolver` gets you a file path; parsing real `.d.ts`
ambient declarations and handling augmentation/merging across packages is
substantially more work than the Stage 2 foundation and isn't needed to
check most everyday application code (as opposed to library-consuming code).

---

## Also excluded from staging - JSX/TSX (Section 12), JS interop (Section 13)

Depend on module/scope concepts; picked up after the module ecosystem work
above, not sequenced here.

---

## Stage dependency summary

| Stage | Depends on | Parallel with |
|---|---|---|
| Engine E0 - safety net | none | any |
| Engine E1 to E2 - cross-file gaps, graph shape | E0 | each other |
| Engine E3 to E4 - cycle unit, scheduler | E2 | - |
| Engine E5 to E6 - config, library and ambient | E3 | each other |
| Engine E7 to E8 - query layer, limits | E4 | E8 runs alongside all |
| 1 - Generics completion | none | 2 |
| 2 - Module resolution (foundation) | none | 1 |
| 3 - Tuples & intersections | none | 4 |
| 4 - Narrowing upgrades | none (loosely 3) | 3 |
| 5 - keyof / indexed access | 3 | - |
| 6 - Mapped & conditional types | 5 | - |
| 7 - Stdlib utility types | 6 | - |
| 8 - Overload resolution | none (ordered after 6) | 3-6 |
| 9 - Contextual typing | none (ordered last) | any |
| Full module ecosystem (.d.ts, augmentation) | 2 | tracked separately |
| JSX / JS interop | module ecosystem | tracked separately |
