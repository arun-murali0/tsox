# Architecture and Maintainability

tsox is a semantic checker built on top of Oxc. The architecture deliberately avoids rebuilding the TypeScript parser/front end and avoids copying the scale or internal structure of larger TypeScript compiler projects.

## Design goal

The core question is:

> Can a Rust-native TypeScript semantic layer remain small, understandable, testable, and extensible while consuming mature front-end infrastructure?

The answer should come from the code and tests, not from an architecture diagram.

## Ownership boundaries

```text
TypeScript source
       │
       ▼
      Oxc
       │
       ├── parser
       ├── AST
       ├── spans
       └── semantic symbols/scopes/references
       │
       ▼
    bridge/
       │
       ├── declaration traversal
       ├── expression checking
       ├── statement checking
       ├── control-flow integration
       └── Oxc-specific details
       │
       ▼
 semantic/
       │
       ├── semantic queries
       └── generic algorithms
       │
       ├───────────────┐
       ▼               ▼
 TypeArena        TypeNamespace
       │               │
       └───────┬───────┘
               ▼
          Subtyping
               │
               ▼
        diagnostics /
        semantic facts /
        future typed IR
```

### Oxc owns the front end

Oxc is responsible for the infrastructure we deliberately do not want to recreate:

- TypeScript/JavaScript parsing;
- AST representation;
- source spans and syntax data;
- semantic symbols, scopes, and references where consumed.

tsox should interact with these through a narrow bridge.

### Bridge owns AST-facing behavior

`src/bridge/` translates Oxc structures into semantic operations.

Expression and statement modules are organized by feature:

```text
bridge/expressions/
bridge/statements/
```

These modules should know about Oxc nodes. They should not become the home for reusable type algorithms.

### Semantic core owns meaning

The semantic core operates on semantic representations such as `TypeId`, not AST nodes.

Current responsibilities include:

- type storage;
- declaration/type resolution;
- assignability and subtype relations;
- generic inference and substitution;
- control-flow narrowing.

This separation makes semantic algorithms reusable by future tooling and typed-IR consumers.

## Module responsibilities

| Module | Responsibility |
| --- | --- |
| `arena.rs` | Allocate and store semantic types |
| `types.rs` | Define the semantic `Type` model |
| `namespace.rs` | Resolve declarations and named types |
| `symbol_map.rs` | Associate Oxc symbols with semantic types |
| `subtyping.rs` | Centralize subtype and assignability rules |
| `type_annotation.rs` | Convert TypeScript type annotations into semantic types |
| `bridge/` | Traverse Oxc AST and coordinate checking |
| `bridge/narrow.rs` | Maintain TypeScript-aware flow narrowing |
| `semantic/queries.rs` | Stable read-only semantic relation boundary |
| `semantic/generics.rs` | Generic inference, substitution, and related semantic helpers |
| `explain.rs` | The chain of reasons tsc prints under a not-assignable error |
| `type_display.rs` | Print a type the way tsc does in a message |
| `lib_global_types.rs` | Generated list of the global type names tsc knows, so a library name is not reported as unknown |
| `keys.rs` | Stable `ModuleKey`, `DeclKey`, `SigHash` and `NodeHash` |
| `stable_hash.rs`, `stable_walk.rs` | The canonical node encoding and its Merkle hash, and the arena walk that feeds it |
| `portable.rs` | Declarations as arena-free data, the importer, and the `ImportSource` a unit reads its imports through |
| `unit.rs` | A file as a unit of work: `UnitContext`, `UnitOutput` and `Worker` |
| `bridge/declare.rs` | The declare pass: register a file's top-level declarations |
| `bridge/imports.rs`, `bridge/exports.rs` | Bind the names a file imports; collect what it exports as portable declarations |
| `bridge/unresolved_names.rs`, `bridge/unreachable_code.rs` | One pass for unknown type names, one for unreachable code |
| `diagnostics.rs` | The public `Diagnostic`/`Severity` representation consumers see |
| `diagnostic_codes.rs` | Stable `TSR####` identifiers, tsox's own namespace |
| `diagnostic_messages.rs` | Pairs a code with its message text, one constructor per diagnostic kind |
| `diagnostic_view.rs` | Converts raw byte offsets into line/column for display |
| `fxhash.rs` | In-tree FxHash reimplementation, used where a non-DoS-resistant hasher is acceptable |
| `line_index.rs` | Source-position conversion |
| `project.rs` | Path to `FileId` identity for the files of a project |
| `incremental.rs` | Opt-in Salsa input (`SourceFile`) and first query; unchanged saves start no revision |
| `scratchpad.rs` | Opt-in bump scratch for one worker; accepts only `Copy` values |
| `module_resolution/` | Opt-in project layer: resolver wrapper, import discovery, module graph with cycles and layers, parallel project check that passes each layer's exports to the next, change detection |
| `wasm.rs` | WASM-facing adaptation |

## Feature modules

Large AST-facing modules are split by meaningful reasons to change.

Expressions:

```text
expressions/
├── assignment.rs
├── binary.rs
├── calls.rs
├── core.rs
├── elaborate.rs
├── excess.rs
├── functions.rs
├── logical.rs
├── members.rs
├── objects.rs
└── mod.rs
```

Statements:

```text
statements/
├── classes.rs
├── control_flow.rs
├── functions.rs
├── patterns.rs
├── support.rs
├── variables.rs
└── mod.rs
```

The `mod.rs` files provide dispatch and narrow facades. Sibling implementation modules should not depend on each other's private implementation details unless there is a real semantic reason.

## Why diagnostics are four files, not one

Diagnostics started as a single `Diagnostic` struct in `diagnostics.rs`: a severity,
a code, a message, a span. As the checker grew past a handful of diagnostic kinds,
three separate concerns that had been living inside that one struct/module were
pulled apart, each for a different reason:

```text
diagnostic_codes.rs      -> what stable identity does this diagnostic have?
diagnostic_messages.rs   -> what code + text belongs to this diagnostic kind?
diagnostic_view.rs       -> where in the source does this diagnostic point?
diagnostics.rs           -> the public Diagnostic/Severity shape itself
```

**Codes are their own namespace.** `diagnostic_codes.rs` assigns every diagnostic a
stable `TSR####` identifier. This is deliberately *not* TypeScript's own `TS####`
numbering — even where a tsox diagnostic describes a condition `tsc` also
reports (an argument arity mismatch, say), the number is not meant to imply
parity with a specific `TS####` code. Keeping the identifier space separate means
tsox's diagnostic catalog can grow, split, or renumber without silently
claiming compatibility it hasn't earned. The compatibility harnesses
(`bin/compare-tsc.rs`, `scripts/compare-local.sh`) compare by line presence, not
by code equality, precisely because of this.

**Message construction is centralized so one diagnostic kind has one source of
truth for its text.** `diagnostic_messages.rs` pairs a code with its message
through exactly one constructor per kind (see the `messages` module there), so
the wording for "argument type mismatch" is written once and reused everywhere
that diagnostic can fire, rather than each call site composing its own string
and drifting from its siblings over time.

**Presentation is separate from storage.** `Diagnostic` itself stores raw byte
offsets, not line/column — those are cheap to produce during checking and don't
need a `LineIndex` lookup unless something is actually about to display the
diagnostic. `diagnostic_view.rs` does that conversion on demand, so the hot
checking path never pays for a presentation concern it doesn't need yet.

The result is the same principle the rest of this document applies everywhere
else: each file answers one question, and nothing downstream needs to know how
the others are implemented to consume a `Diagnostic`.

## Semantic query boundary

`semantic::queries` provides a deliberately small read-only interface between AST-facing checking code and semantic relations.

Current operations include:

```text
is_assignable(source, target)
is_subtype(source, target)
```

The boundary is intentionally incomplete. New operations should be added when they represent meaningful semantic questions whose implementation should be hidden from bridge modules.

Likely future queries include:

```text
property_type(...)
call_signature(...)
construct_signature(...)
resolve_type(...)
infer_generic_call(...)
instantiate(...)
```

These should be introduced only when the underlying semantic operation is mature enough to justify a stable API.

## Generic semantics

Generic algorithms have been moved out of expression traversal into `semantic/generics.rs`.

The dependency direction is:

```text
call expression
      ↓
generic semantic helper
      ↓
TypeArena / TypeNamespace / Subtyping
```

The goal is to prevent generic inference from becoming a collection of call-expression special cases.

As generic semantics grow, this file can become a `semantic/generics/` module without changing the bridge contract.

## Sessions, file identity and metrics

`TypeChecker::check_source` is the one-shot entry point: it owns a throwaway arena. A
caller that re-checks a file keeps a `CheckSession` (`TypeChecker::session(file_id)`),
which holds the reusable arena and moves it into each check and back, with its capacity.
The arena is cleared at the start of every check, so no type, name, intern entry or cached
answer from the previous file is visible to the next one. The reusable state is in the
session and not in `TypeChecker`, so the checker has no interior mutability and
independent sessions can run side by side.

`FileId` is a plain index. It is part of `TypeParameterId`, so two files' generic
parameters at the same byte offset stay distinct once they share semantic state.
`ProjectFiles` owns the path-to-`FileId` mapping and does nothing else: no parsing, no
resolution, and it never canonicalizes a path, because that would make handing out an id a
filesystem operation. Module resolution attaches to it from the outside.

After a successful check the session keeps `CheckMetrics`: arena sizes, relation-cache hits
and misses, and the namespace's instantiation counts. They are counts of structures the
code already maintains, read after the fact, so collecting them costs nothing.

## CheckContext

`CheckContext` represents one mutable checking session.

It currently contains:

- `TypeArena`;
- `TypeNamespace`;
- `SymbolTypeMap`;
- diagnostics;
- file information;
- narrowing state;
- current return type;
- current class instance.

This is intentionally simple at the current scale.

The future goal is to keep session state clearly separated from reusable semantic services. We should not prematurely turn every field into a trait or service object.

## Dependency rules

Prefer:

```text
Oxc AST
   ↓
bridge feature
   ↓
semantic service
```

Avoid:

```text
semantic service
   ↓
Oxc AST traversal
```

and avoid uncontrolled sibling coupling:

```text
expression feature A
   ↓
private implementation of expression feature B
```

Use a parent-module facade when a shared operation genuinely belongs to the feature family.

## SOLID, pragmatically

The project applies SOLID through Rust's natural boundaries rather than through inheritance.

- **Single responsibility:** modules represent meaningful semantic responsibilities.
- **Open/closed:** new behavior is normally added to a focused feature module and semantic service.
- **Dependency inversion:** semantic algorithms depend on semantic types rather than Oxc AST nodes.
- **Interface segregation:** public facades expose only stable operations that consumers need.
- **Liskov substitution:** `Type` variants are interpreted through centralized semantic relations rather than ad-hoc AST checks.

The goal is maintainability, not a maximum number of abstractions.

## Future project-scale checking

The desired ownership model is:

```text
Project
  │
  ├── immutable module/symbol/type index
  │
  ├── file A → CheckContext
  ├── file B → CheckContext
  └── file C → CheckContext
```

Worker-local state should include diagnostics, narrowing, current function/class state, and temporary inference state.

Shared project information should become immutable or snapshot-based after indexing.

Most of this exists behind the `module-resolution` feature: `ModuleGraph` is built synchronously (it assigns `FileId`s and edges), and `check_project` checks each dependency layer in parallel, each file as a unit with its module key and an `ImportSource`, one `Worker` per thread, sorting the report by `FileId`. What a layer exports is collected before the next layer starts, so an import is looked up in the file it names, and the members of a cycle are checked again until their exports settle. See [module-resolution](architecture/module-resolution.md) for the graph and [cross-file-types](architecture/cross-file-types.md) for what crosses files.

The eventual parallel boundary is the file/program/session, not individual AST nodes. Do not add locks to the semantic hot path merely to prepare for multithreading.

## Semantic facts and tooling

The checker should eventually produce reusable semantic facts:

```text
semantic result
├── diagnostics
├── symbol information
├── inferred types
└── semantic facts
```

Tooling consumes those facts:

```text
semantic facts
├── hover
├── CodeLens
├── inlay hints
└── LSP
```

Editor presentation must remain outside AST traversal and core type algorithms.

## Typed IR

Typed IR is a later consumer:

```text
Oxc AST
   ↓
semantic checking
   ↓
typed representation
   ↓
HIR / SIR when justified
   ↓
backend
```

The IR should consume established semantic facts. It should not become a parallel type system.

## Reference architecture

Larger Rust TypeScript implementations such as `tsz` are valuable references for understanding:

- solver boundaries;
- generic inference;
- relation handling;
- control-flow modeling;
- project-scale state;
- architectural failure modes.

They are references, not specifications.

The project should adopt an idea because it solves a demonstrated tsox problem, not because another compiler has a similarly named module.
