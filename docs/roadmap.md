# Roadmap

This roadmap describes the semantic capabilities the project is building toward. The names are capability-based rather than numbered releases, so the repository remains understandable without knowing its development history.

## Current baseline

The semantic frontier is **the project engine and breadth of the type system**. A project is checked as a set of files: imports of every declaration kind cross files, cycles settle, `tsconfig.json` selects files and steers resolution, globals and module augmentations are visible without an import, and only what a change reaches is checked again. The pieces are mapped in [project-engine](architecture/project-engine.md). What the checker gets right and wrong today is measured against `tsc` in [language-coverage](language-coverage.md), and on the compiler's own tests in the [conformance baseline](conformance/baseline-part01-05.md).

The completed foundation includes:

- Oxc-backed parsing and semantic information;
- primitive and function types, structural objects, arrays, tuples, unions and intersections, aliases and interfaces;
- declaration resolution and circular-reference protection, with declarations as `Ref` slots and names on their own nodes;
- literal widening and control-flow narrowing, including discriminated unions, loops, joins, type predicates and assertion functions;
- classes, inheritance, constructors, methods and `this`;
- generics on functions, interfaces, aliases and classes, with inference, constraints, defaults, recursive shapes and variance-based relations;
- `keyof`, indexed access, conditional types with `infer`, overload sets and the `Partial` and `Pick` utility types;
- `async` and `await`, and loosely typed array and Promise members;
- stable keys, portable declarations and a project check that settles import cycles and publishes each file's exports to its importers;
- a ready-queue scheduler, partial export lists with named reasons, resource limits that report instead of guessing, and a query layer for incremental and watch mode.

## Next

The work that is open, in the order the evidence suggests. Each item says where its detail lives.

1. **Fix the false positive on static members.** Reading `A.s` or `A.make()` through the class name is reported as a missing property on valid code. It is the only false positive found in [language-coverage](language-coverage.md), so it comes first.
2. **Enforce the class and object rules that need no new type machinery:** abstract instantiation, `private` and `#private` access, `implements`, override compatibility, `readonly` writes, use before assignment, iterability in `for ... of`, `satisfies`, and namespace members. Each has a probe that turns to a match when it is done.
3. **Resolve the types that are still unresolved:** mapped types, template literal types, index signatures and `Record`. Today the checker warns that the annotation is not resolved and says nothing more.
4. **Give the standard library real shapes.** The TypeScript 6.0.3 lib files are vendored, loaded through the project provider, checked, and cached (loading, the DOM deferral and the cache are done; see section 9 of [library-and-ambient-declarations](architecture/library-and-ambient-declarations.md)). What is left is generic inference, which replaces the loose `any` that `array.map` and `promise.then` return where inference cannot yet carry a library signature.
5. **Shrink the partial export lists.** One reason, `UnrepresentableDeclaration`, accounts for most of them, and declaration files without export statements have no semantics yet. See [partial-results](architecture/partial-results.md).
6. **Close the conformance gaps in order of frequency**, and the hangs the baseline found, then reconcile the baseline's two sets of percentages.
7. **Read the flow graph for narrowing.** The graph is built for every function body and nothing reads it yet. See [flow](architecture/flow.md).
8. **Engine measurements and limits:** parallel discovery (needs a multi-core measurement), a cheaper second pass for projects whose scripts use each other's names, budgets for recursive work, and the cubic `else if` chain. See [safety-net-and-limits](architecture/safety-net-and-limits.md) and [scheduler](architecture/scheduler.md).

The cross-file algorithms belong beside the portable declarations. AST traversal belongs in `bridge/`.

## Order of the type-system work

The order is based on semantic dependencies, not on TypeScript syntax order:

```text
the class and object rules          (no new type machinery)
    ↓
mapped types + template literal types + index signatures
    ↓
utility types built on them (Record, Required, Omit, ...)
    ↓
real library shapes through the provider
    ↓
stronger control-flow semantics, reading the flow graph
```

## Semantic facts

Once inferred types and symbol information are stable, the checker should expose reusable semantic facts:

```text
checker
  ↓
semantic result
  ├── diagnostics
  ├── symbol information
  ├── inferred types
  └── semantic facts
```

Those facts can later power hover, CodeLens, inlay hints, and LSP features without coupling editor presentation to the checker.

## Typed IR

A typed intermediate representation should come after the semantic model is mature:

```text
Oxc AST
  ↓
checked semantic model
  ↓
typed IR
  ↓
HIR / SIR where justified
  ↓
future backends
```

The IR should represent information the semantic core can actually guarantee. It should not become a second, parallel type system.

## Project scale and parallelism

Project-level work should eventually separate immutable indexed information from worker-local checking state:

```text
Project
  ↓
immutable project index
  ├── file A → CheckContext
  ├── file B → CheckContext
  └── file C → CheckContext
```

The graph, layering and parallel project check are in place behind the `module-resolution` feature (see [module-resolution.md](architecture/module-resolution.md)). The part that makes the graph matter to the checker is in place too: each checked file exports an owned table, and import sites look into the file they name. What remains is packages, `paths` and re-checking only what changed (see the list above).

Parallelism belongs at the file/program/session boundary. The current checker should not gain locks merely in anticipation of this future.

## Performance

Every major semantic change should be checked against:

- Criterion benchmarks;
- IAI callgrind benchmarks;
- heap profiling where relevant;
- representative fixtures;
- the full regression suite.

Correctness comes first. Once a semantic operation is stable, profiling should identify whether caching, canonicalization, allocation reduction, or algorithmic changes are justified.

## Reference projects

`tsz` is useful as a reference for semantic algorithms, scaling problems, solver boundaries, and architectural lessons.

It is not the architecture specification for tsox.

Oxc remains the front-end foundation. tsox owns the semantic layer built on top of Oxc.
