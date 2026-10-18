# Roadmap

This roadmap describes the semantic capabilities the project is building toward. The names are capability-based rather than numbered releases, so the repository remains understandable without knowing its development history.

## Current baseline

**Cross-File Types** is the current semantic frontier: a type written in one file is used in another, imports are diagnosed, and the members of an import cycle settle (see [cross-file-types](architecture/cross-file-types.md)).

The completed foundation includes:

- Oxc-backed parsing and semantic information;
- primitive and function types;
- structural objects, arrays, unions, aliases, and interfaces;
- declaration resolution and circular-reference protection;
- literal widening;
- control-flow narrowing;
- classes, inheritance, constructors, methods, and `this`;
- expression and statement checking;
- parameters and destructuring;
- generics on functions, interfaces, aliases and classes, with inference, constraints, defaults and recursive shapes;
- narrowing that follows control flow, including discriminated unions, loops and joins;
- intersection types and calls through an intersection of functions;
- declarations as `Ref` slots with names on their own nodes;
- stable keys, portable declarations and a project check that passes each layer's exports to the next.

## Next: closing the cross-file gaps
The generic core that this section used to describe is done. The next work finishes what crossing files started, without another repository-wide rewrite.
Targets:
1. export classes, enums and overloads, which today set `incomplete` and are left out;
2. `export * as ns`, default classes, default expressions and anonymous defaults;
3. read packages, so an import from `node_modules` binds something and a bare specifier can be diagnosed;
4. tsconfig `paths` and `baseUrl`;
5. re-check only the changed files and their dependents, through the Salsa layer that exists and is not yet used;
6. decide on purpose between one unit per cycle and the current iteration of its members (decision 4 in [decisions](decisions.md)).
The cross-file algorithms belong beside the portable declarations. AST traversal belongs in `bridge/`.

## After the cross-file foundation

The preferred order is:

```text
Cross-File Types
    ↓
keyof + indexed access
    ↓
overloads + contextual typing
    ↓
mapped types
    ↓
conditional types + infer
    ↓
utility types
    ↓
stronger control-flow semantics
```

The order is based on semantic dependencies, not on TypeScript syntax order.

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
