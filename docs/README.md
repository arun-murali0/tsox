# tsox Documentation

This directory is the maintained technical guide for tsox.

The repository uses **semantic milestone names** instead of opaque numeric development labels. A contributor should be able to understand what a test suite or document covers from its name alone.

## Current semantic baseline

```text
Foundation
    ↓
Structural Types
    ↓
Literal Widening
    ↓
Control-Flow Narrowing
    ↓
Classes and Inheritance
    ↓
Expression and Program Checking
    ↓
Parameters and Destructuring
    ↓
Generics (functions, interfaces, aliases, classes, recursion, builtins)
    ↓
Narrowing completion, sessions and file identity (joins, discriminated unions, `in`/`instanceof`, `CheckSession`)
    ↓
Module Resolution (opt-in `module-resolution` feature: resolver, module graph, cycles, layers, project check)
    ↓
Architecture Foundation (opt-in `incremental`, `scratchpad` and `mimalloc` features)
    ↓
Declaration Slots and Named Nodes (`Ref` slots, `Named` and `App` nodes)
    ↓
Intersection Types
    ↓
Cross-File Types (stable keys, portable declarations, units, imports and exports) ← current baseline
```

These names describe capabilities. They are not release versions and should not be interpreted as a versioning scheme.

For the full, item-by-item picture of what's checked and what isn't across the entire TypeScript type system (not just this milestone list), see [`checklist.md`](../checklist.md) at the repository root — it's the actively maintained source of truth for current coverage, kept more granular than this milestone list.

## Milestones

| Milestone | Purpose | Tests | Documentation |
| --- | --- | --- | --- |
| Foundation | Establish the end-to-end checker, primitive types, basic functions, diagnostics, and honest unsupported-feature handling. | `tests/foundation.rs` | [foundation](foundation.md) |
| Structural Types | Add objects, arrays, unions, aliases, interfaces, declaration resolution, structural subtyping, and function arity. | `tests/structural_types.rs` | [structural-types](structural-types.md) |
| Literal Widening | Preserve literal information while modeling widening behavior. | `tests/literal_widening.rs` | [literal-widening](literal-widening.md) |
| Control-Flow Narrowing | Refine types across branches and flow paths. | `tests/control_flow_narrowing.rs` | [narrowing](architecture/narrowing.md) |
| Classes and Inheritance | Model class members, constructors, inheritance, static state, and `this`. | `tests/classes_inheritance.rs` | [classes-inheritance](classes-inheritance.md) |
| Expression and Program Checking | Expand expression inference, calls, members, logical operations, loops, switches, assertions, and program-level checking. | `tests/expression_program_checking.rs` | [expression-program-checking](expression-program-checking.md) |
| Parameters and Destructuring | Model parameter metadata and binding/destructuring semantics. | `tests/parameters_destructuring.rs` | [parameters-destructuring](parameters-destructuring.md) |
| Generics | Type parameters on functions, interfaces, aliases, and classes: inference, substitution, constraints, defaults, explicit type arguments, recursive generic shapes, and builtin generics (`Array<T>`, opaque `Promise<T>`). One consolidated document, not split by tier — see it for why. | `tests/generics_tier1.rs`, `tests/generics_tier2.rs`, `tests/generic_classes.rs`, `tests/recursive_types.rs`, `tests/builtin_generics.rs`, `tests/interface_methods.rs` | [generics](architecture/generics.md) |
| Intersection Types | `A & B`: ordered identity, construction rules checked against tsc, lazy reduction of discriminant conflicts, relations, member access, calls and generics. 25 + 10 unit tests, 17 fixtures. Calls through an intersection of functions and TS2590 follow tsc; narrowing does not see intersections. | `tests/intersection_types.rs`, `tests/intersection_overloads.rs`, unit tests in `src/arena.rs` and `src/subtyping.rs` | [intersections](architecture/intersections.md) |
| Declaration slots and named nodes (stage 1, steps 1 and 2) | Interfaces, classes and object-literal aliases are `Ref` slots that are never rewritten, and a name lives on a `Named` or `App` node instead of on an id. `set`, placeholders, `display_names`, `make_unique`, `duplicate_named` and the settled-display check are gone. Object properties are not yet bump slices (step 3). | unit tests in `src/arena.rs` and `src/type_display.rs`; the existing fixtures unchanged | [arena](architecture/arena.md) |
| Module Resolution | Resolve imports with `oxc_resolver`, build a graph indexed by `FileId`, detect cycles, order files into dependency layers, check a project in parallel, and detect changed files. Behind the `module-resolution` feature. What the checker does with the files it is handed is the next row. | `tests/module_resolution.rs` (run with `--features module-resolution`) | [module-resolution](architecture/module-resolution.md) |
| Architecture Foundation | The dependencies the design documents name, each behind its own feature and not yet used by the checker: a Salsa input and first query, a bump scratchpad that takes only `Copy` values, and mimalloc for the binary. | unit tests in `src/incremental.rs`, `src/scratchpad.rs` | [foundation-dependencies](architecture/foundation-dependencies.md) |
| Cross-File Types | A declaration gets an identity that survives a run and moves between arenas as data. A file is a unit with an owned output, a layer's exports reach the files that import it, imports are diagnosed (TS2307, TS2305, TS1192) against complete export lists, and cycles are checked again until their exports settle. Classes, enums, overloads and packages are not exported yet. | `tests/unit_exports.rs`, `tests/unit_imports.rs`, `tests/project_imports.rs`, `tests/import_diagnostics.rs`; golden hashes in `src/stable_hash.rs` and `src/portable.rs` | [cross-file-types](architecture/cross-file-types.md) |

Later work extends these milestones rather than adding a tier: `tests/narrowing_advanced.rs` and `tests/narrowing_complete.rs` (switch, `&&`/`||`, loops, joins, `in`, `instanceof`), `tests/generics_bounds.rs` (literal-preserving inference, explicit type arguments), `tests/declaration_collisions.rs`, `tests/union_member_access.rs` and `tests/check_session.rs` (sessions, metrics, file identity), `tests/unreachable_scopes.rs` (dead code in every construct, checked against tsc line for line) and `tests/everyday_patterns.rs` (empty arrays, parameter properties, boolean discriminants, generic callbacks).

A handful of smaller, cross-cutting suites support these milestones rather than being their own tier: `tests/call_arity.rs`, `tests/void_and_never.rs`, `tests/unreachable_code.rs`, `tests/named_types_in_messages.rs`, `tests/diagnostic_codes.rs`, `tests/misc_fixes.rs`, and `tests/subtype_cache_stress.rs` (a performance/correctness regression suite, not a feature milestone).

## Architecture case studies
`docs/architecture/` holds one document per subsystem, each written as what was planned, the issue that was found, what was changed and what it gained:
- [arena](architecture/arena.md): ids, interning, `Ref` slots, named nodes, digest collisions, the performance phases.
- [narrowing](architecture/narrowing.md): the overlay, joins, discriminants, loops and unreachable code.
- [generics](architecture/generics.md): type parameter identity, inference, substitution, bounds.
- [intersections](architecture/intersections.md): ordered identity and lazy reduction.
- [module-resolution](architecture/module-resolution.md): the resolver, the graph, layers and the project check.
- [cross-file-types](architecture/cross-file-types.md): keys, portable declarations, units, imports and exports.
- [foundation-dependencies](architecture/foundation-dependencies.md): Salsa, bump allocation, mimalloc.

## Why it is built this way

[case-study.md](case-study.md) collects the design decisions in one place: the problem each solves, what was rejected, and how it was checked. Start there when a choice looks odd. [decisions.md](decisions.md) is the shorter list of what the design documents left open and how each was closed.

## Architecture

Read [architecture.md](architecture.md) for the current ownership model.

The short version is:

```text
Oxc
  ↓
bridge
  ↓
semantic layer
  ↓
TypeArena / TypeNamespace / Subtyping / Generics / Narrowing
  ↓
diagnostics + future semantic facts
  ↓
tooling / typed IR
```

Oxc remains the front-end foundation. tsox implements the semantic behavior that sits on top of it.

## Roadmap

[roadmap.md](roadmap.md) covers the project's longer-term direction (semantic facts, typed IR, project-scale checking, performance philosophy) that doesn't change week to week.

For the actual current build order — what's next and why, in priority order — see [`BUILD.md`](../BUILD.md) at the repository root, which is derived from and stays in sync with `checklist.md`. Both are updated more frequently than this documentation and are the ones to check for "what's actually being worked on right now."

## Testing and performance

Read [testing.md](testing.md) for:

- semantic fixture conventions;
- regression policy;
- formatting/check/lint/test commands;
- Criterion and IAI benchmarks;
- heap profiling;
- compatibility harness expectations.

## Design boundaries

Read [purpose-and-overdesign.md](purpose-and-overdesign.md) for:

- why tsox builds on Oxc;
- what tsox owns;
- what it deliberately does not rebuild;
- how larger projects such as tsz are used as references;
- why premature abstraction and premature multithreading are avoided.
