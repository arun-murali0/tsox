# tsox architecture: modular, query-driven, ready for emit and IR (revision 2)

> **Snapshot.** File names in this document (`narrow.rs`, `exports.rs`, `declare.rs`, `calls.rs`, `generics.rs`) are the ones before the splits; they are directories now. `new-architecture-progress.md` has the current layout and status.

Revision of the Nov 15, 2026 draft, Nov 16, 2026

The first draft built the skeleton: layers, registries, a semantic model, a host boundary and an emit/IR order. This revision keeps all of it and adds what the draft left out: the type-checking engines (relations, generics, members, narrowing, diagnostics), a single flow architecture, the exact mechanics of per-body reuse, and a migration plan that fixes correctness and incrementality in parallel with the refactors instead of after them.

**Status markers.** *Verified* means read in the source at the working tree this document is based on (`a05b7c7c`, branch `newtypes`). *Hypothesis* means plausible from reading and must be measured before it drives a decision.

## What this revision adds

| Area | First draft | This revision |
| --- | --- | --- |
| Type-checking logic | Registries for narrowing and calls only | A section on engines: relations, generics and inference, member lookup, narrowing, diagnostic provenance, with one contract per operation |
| Flow | A `flow_facts` table in the model | A section on flow: one graph, one answer type, algorithms for `catch`, `finally`, calls, optional chains and loops, and who owns definite assignment and reachability |
| Per-body reuse | Listed as `signature` and `check_body` queries | Specified as a memo inside the unit: stable item identity, signature and body hashes, a dirty plan, backdating, typed reads |
| Why units recheck | Assumed | Measured first (step 0), with four named suspects |
| Migration | Refactors first, behavior later | Four parallel tracks: structure, correctness, incremental, consumers |
| Claims | Unmarked | Marked verified or hypothesis |

## Goals and non-goals

The goal is to make every piece of tsox a small query with one owner, so a contributor can add a check without reading the whole checker, and the same semantic facts can feed diagnostics, a JS emitter, WASM and later a native IR.

Goals:

- **Demand-driven:** nothing is computed unless a query asks for it. Incremental: an edit re-runs only the queries whose inputs changed, and an edit that changes no meaning (a comment, a moved line) re-runs none past the parse.
- **Modular:** each language feature (a narrowing rule, a call form, a declaration kind) lives in its own module with its own tests, behind a stable interface.
- **Decoupled:** the checker, the emitter, the LSP and WASM depend on a shared semantic model, never on each other.
- **Cache-friendly:** every query is pure and deterministic, so results can be memoized in memory and on disk, and cut off early when an output is unchanged.
- **Correct:** conformance with tsc is a goal with its own work items, not only a gate (currently about 51% of the 10,083-file corpus, target 75%).
- **Enterable:** a new contributor adds one behavior by touching one module, one table line and one fixture.
- **Measurable:** every step is gated by tsc conformance, tests, determinism and the 5000 ms per-file limit.

Non-goals:

- Rewriting what works. The Salsa-based project layer, the per-unit arena and the Oxc front end stay.
- Building a new parser or an AST of our own.
- Replacing `oxc_cfg`. Oxc supplies the block skeleton of each body; tsox adds the TypeScript-specific layer on top (section on flow).
- A shared global type interner on speculation. It is revisited only when a benchmark says portable conversion dominates.
- Promising a native compiler for all of TypeScript. The IR section says which subset is realistic.

## Where the code stands today

The foundation is close to the target: the layers exist, but the middle of the checker is one large walk and several files are too big for a newcomer to enter.

| Crate | Files | Lines | Role today |
| --- | --- | --- | --- |
| tsox-base | 20 | 7,491 | Stable keys, diagnostic codes and the public Diagnostic type |
| tsox-types | 19 | 21,212 | Type arena, subtyping, generics, annotation resolution, display |
| tsox-checker | 47 | 28,358 | `bridge/` (Oxc AST to semantic calls), flow, units, reachability |
| tsox-db | 3 | 2,603 | Salsa inputs and queries: SCC plan, module header, `check_scc`, `export_names`, `file_report` |
| tsox-project | 15 | 8,197 | Project engine, tsconfig, module resolution, scheduler |
| tsox-lsp | 14 | 8,348 | Language server |
| tsox-wasm | 1 | 78 | Thin wasm-bindgen wrapper over the facade |

What already works for us:

- Salsa queries exist at file and import-cycle grain (`check_scc`, `decl_shape`, `export_names`, `file_report`) with early cutoff through stable Merkle hashes (`keys.rs`).
- Declarations cross files as arena-free portable data (`portable.rs`), so each file keeps its own arena.
- The semantic core (`tsox-types`) takes no AST. Generic inference is candidate-based (`GenericInferenceSession`, with priorities), which matches how TypeScript infers. *Verified.*
- A flow table built from `oxc_cfg` plus a TypeScript-specific AST pass, and a lazy backward resolver that already answers many identifier reads. *Verified.*

What holds contributors and the long-term goals back:

- **Large files:** `bridge/narrow.rs` (2,306 lines), `bridge/exports.rs` (2,097), `bridge/expressions/calls.rs` (1,912), `bridge/declare.rs` (1,473), `bridge/syntax_repair.rs` (1,094). *Verified for the first three.*
- **No typed output:** checking returns diagnostics, not a table of node to type.
- **File-grain recomputation:** one edit inside a function body re-checks the whole file.
- **Layers by convention:** nothing fails the build when `bridge` code reaches into an internal it should not.
- **Host access:** `std::fs` appears in `tsox-db` (`project.rs`), `tsox-base` (`standard_lib.rs`) and across `tsox-project/module_resolution/*` (the draft missed this one); the only `Host` trait is in the LSP (`navigate.rs`). *Verified.*
- **Three flow mechanisms side by side.** *Verified.* (1) `reachability.rs` (2.2k lines) replays tsc's binder rules over the AST for dead code, definite assignment and return paths, and does not use `oxc_cfg`. (2) The forward overlay (`narrow.rs` with `statements/control_flow.rs`) carries a `NarrowState` through the walk and owns every narrowing rule. (3) `flow.rs` with `bridge/flow_shadow.rs` is the graph and the lazy resolver, active by default but answering only reads whose declared type is a union or `unknown`, and handing back to the overlay for `catch` and `finally`, `??`, compound assignment and destructuring, variables of an outer function, and every variable in a file that declares an assertion function. The three can disagree, and no one owns the answer.
- **Order-dependent inferred returns.** A caller checked before the callee's body is walked sees a placeholder `any`, and a mutual cycle gets the same placeholder without a diagnostic (note on `inferred_returns` in `bridge/context.rs`). *Verified.*
- **Comments and docs that no longer match the code.** `parse.rs` and the `flow.rs` header say nothing consumes the graph for narrowing; `flow_shadow.rs` says it is active by default. *Verified.*

### Why most units recheck: four suspects

*Hypothesis.* Read from `check_scc`, the unit plan and `ProviderSet`; none is measured.

1. **Provider digest in every unit key.** Every unit is keyed by an `env` digest of the ambient providers' exports. A file with no `import` or `export` is a script and counts as a global provider, so one edit to a script changes `env` for every unit.
2. **Whole configuration in every plan.** Each unit's plan carries the compiler options as one text, so changing any option changes every unit even if the unit never read it.
3. **Moving file ids.** A comment in the plan code notes that an id that moves changes the plan of every unit that sees it.
4. **Coarse fallbacks.** When a replayed result does not know its reads, the unit falls back to depending on every direct import's export list.

The measurement that tells which one applies is already available: `IncrementalProject::take_log()` records `scc_run:<id>` per unit that runs. Count the entries after one body edit, after one edit to a script file, and after one option change. This is step 0 of the plan.

## What stays, what grows

This plan only extends. The query-layer and project-engine notes in `docs/architecture/` describe a design that was built and measured, so each existing part is kept and the table says what is added on top of it.

| Existing part | Status | What is added |
| --- | --- | --- |
| Two planes: Salsa decides what reruns, the checker does the work and knows nothing about Salsa (`check_unit_isolated`) | Kept as is | The rule is written into `check-layers`, so the checker can never import Salsa |
| Two drivers: `check_project` for cold CLI runs, `IncrementalProject` for editors and watch | Kept as is | A third consumer, `build`, reads the same results; no new driver |
| `decl_shape`, `export_names`, `export_lookup`, `export_prefix`, `export_complete`, `file_report` | Kept as is | Per-item results (signature, body) sit inside `check_scc`'s output, between `decl_shape` and `file_report` |
| `SourceFile` input with a content hash | Kept as is | The same hash scheme keys the disk cache |
| `module_header(file)` with no offsets | Kept as is | `semantic_model` also strips offsets from its hash |
| Provider queries and the global-scope epoch | Kept as is | The unmemoized second pass for global augmentations is the first candidate for a query |
| `lru = 4096` on `check_scc` | Kept as is | The same knob applies to the per-item memo, with its own default chosen by measurement |
| Cooperative cancellation (`CancelToken`) | Kept as is | The per-item memo checks the token at the same points |
| `set_buffer` and `close_buffer` for editor text | Kept as is | Unchanged |
| Per-worker arena cleared after each unit; library as a name list | Kept, revisited later | A shared library arena is a measured option, not a step |
| `snapshot` feature for checked library files | Kept as is | Becomes the on-disk format for any arena-free query output |
| Safety net: depth guard, size limits, named partial reasons | Kept as is | Every new engine reports through the same partial-result type |
| `GenericInferenceSession` (candidate-based inference) | Kept as is | Wrapped by probes and structured candidate failures (engines section) |
| `flow.rs` table and `oxc_cfg` skeleton | Kept as is | Gains call, optional-chain and exception events (flow section) |
| `reachability.rs` tsc-accurate replay | Kept until proven redundant | Compared against the graph in shadow mode before anything is retired |

## Target layering

Read it top to bottom: consumers depend only on the semantic model, the model is produced by queries, queries run the checker, and the checker runs over Oxc output.

1. **Consumers:** CLI, language server, WASM host, emitter, IR. They read the semantic model and the database, never `bridge` internals.
2. **Semantic model:** node to type, node to symbol, flow facts, constants, module info, diagnostics. New.
3. **Queries:** Salsa at project, file and import-cycle grain (exists), and a per-item memo inside a unit (new).
4. **Checker:** phases and engines over `tsox-types`, with registries for narrowing rules, call forms and passes.
5. **Front end:** Oxc parse, semantic and `oxc_cfg`.

`tsox-types` takes no AST and `tsox-base` carries the one `Host` trait, so the lower layers build for WASM unchanged.

## Query graph and caching rules

A query is a pure function from inputs to an output that is cheap to compare. Today's file and cycle grain stays; two finer levels are added, **signature** and **body**, so an edit inside a function body does not invalidate its importers.

### Two levels of incrementality

| Level | Grain | Mechanism | Status |
| --- | --- | --- | --- |
| 1 | Project, file, import cycle | Salsa tracked queries with early cutoff | Exists |
| 2 | Declaration | `decl_shape`, `export_*` queries; a reader asks for exactly the names it uses | Exists |
| 3 | Item: signature and body | A memo inside `check_scc`, kept beside Salsa (like `SettledCycle`), validated by recorded reads | New |
| 4 | Expression | Not planned; positions already checked come from the semantic model | Deferred |

Why level 3 is a memo and not a Salsa query per body: a body is checked in its file's arena and mutable context, so a separate Salsa query would have to rebuild that arena on every call. The memo keeps Salsa at unit grain, where the arena lives, and gets the same red-green behavior for bodies. If a later change makes the arena rebuild cheap or shared, the memo can be promoted to tracked queries without changing the keys.

| Query or fact | Input | Output | Cut off when |
| --- | --- | --- | --- |
| `parse(file)` | source text | AST handle, syntax diagnostics, repair notes | text unchanged |
| `module_header(file)` | parse | imports, exports, declaration names | header hash unchanged |
| `decl_shape(decl)` | header, provider shapes | portable declaration (exists) | shape hash unchanged |
| `signature(item)` | item tree, imported shapes | portable item signature, no body | signature hash unchanged |
| `check_body(item)` | signature, body AST, reads | diagnostics and typed facts for one body | body key unchanged (own text plus everything read) |
| `check_scc(scc)` | member files | export tables for import cycles (exists) | export hashes unchanged |
| `file_report(file)` | all bodies of a file | final diagnostics in tsc order | list unchanged |
| `semantic_model(file)` | bodies, signatures | node to type and symbol table for consumers | model hash unchanged |

### Caching rules every query must follow

1. **Pure:** reads only its arguments and Salsa inputs; no globals, no clock, no environment, no file system.
2. **Deterministic:** the same inputs give byte-identical output, including diagnostic order.
3. **Arena-free outputs:** a query returns portable data, never a `TypeId` from another unit's arena.
4. **Hashed at the boundary:** each output carries a stable hash; downstream queries depend on the hash, not the content.
5. **Bounded:** each query respects the depth and size limits and returns a named partial result instead of guessing.
6. **Disk cache by hash:** arena-free outputs are stored through the `snapshot` feature, keyed by content hash and tsox version.
7. **Typed reads:** a check records what it asked of the world in one typed log (below), never as ad hoc lists.

### Item identity and the item tree

A per-body memo needs a name for an item that survives edits elsewhere in the file.

- **`DefKey`:** module, a path of names down to the item, and a disambiguator for the nth item of the same name in the same scope (overloads, merged declarations). Never a byte offset or an AST-order index. `DeclKey` already does this for exports; `DefKey` extends it to nested items such as methods and class members.
- **Item tree:** a pure function of the source, with no arena, that lists each item with a **signature hash** and a **body hash**, computed by a structural visitor that ignores spans. A comment, a blank line or an edit above the item changes neither. The signature of an unannotated `const x = e` includes `e` (the type comes from it); the signature of a function without a return annotation does not include its body, because its inferred return is a separate fact (below).

### The typed read log

The checker already records declaration reads and import reads, and `check_scc` replays them as tracked reads. One typed log generalizes both:

| Read | Meaning |
| --- | --- |
| `Decl`, `ExportName`, `ExportList`, `ExportComplete` | what exists today |
| `Item(DefKey)` | a same-file item's signature |
| `ReturnOf(DefKey)` | a same-file function's inferred return |
| `Option(OptionKey)` | a compiler option the check consulted |
| `LibFile(LibId)` | a library declaration file, for profile-aware libs |

The `Option` read removes the "any option change reruns every unit" suspect: a unit that never read `exactOptionalPropertyTypes` does not depend on it.

### Deciding what to rerun

After an edit the memo plans a recheck in four passes. This is rustc's try-mark-green, done by hand inside the unit.

1. **Direct edits.** An item is dirty if it is new, its signature hash changed (signature dirty), or only its body hash changed (body dirty). A removed item counts as a signature change for whoever read it. The module's own top-level statements are one more body.
2. **Outside reads.** A body is dirty if any recorded read of another unit, or of an option, now has a different answer.
3. **Same-file propagation to a fixpoint.** A changed signature dirties every body that read it; a changed inferred return dirties every body that read `ReturnOf` it. The set only grows and is bounded by the number of items.
4. **Backdating.** A body that is rerun and produces the same result digest as before stops the propagation: its readers are not dirtied.

The declare pass and signature resolution still run for the file on every change, because they build the arena the bodies need. That cost must be measured (step 0) before the memo is built: the scheme pays off only if bodies dominate.

### Inferred returns as a demand query

Today a caller can see a placeholder `any` for a callee that has not been walked yet. As a demand query, asking for the return of `g` runs `g`'s body if it has not run, and a cycle is detected and reported at the call (the tsc "implicitly has return type any because it references itself" case) instead of being passed off as `any`. This removes the order dependence and gives the memo a clean `ReturnOf` edge.

### Open decision

Per-unit arenas with portable boundaries (today) versus one shared interned type store. Per-unit keeps queries independent and parallel; shared makes cross-body type identity cheap. The plan keeps per-unit arenas and revisits only if body-level measurements show portable conversion dominating, or if IR lowering needs cross-body identity.

## Incremental by construction

Incremental checking and demand-driven checking are one mechanism: a query that is demanded is memoized, and a memo is reused while its recorded reads are unchanged. The editor, watch mode and the language server already run this way through `IncrementalProject`; this plan extends how far down an edit is cut off.

| Step | Query | Stops here when | Status |
| --- | --- | --- | --- |
| 1 | `SourceFile` input | the bytes are equal (hash) | Exists |
| 2 | `parse` and `module_header` | the header has no offsets, so a text-only move leaves it equal | Exists |
| 3 | `decl_shape`, `export_names` | the projection is equal, so importers are untouched | Exists |
| 4 | signature | the signature hash is equal, so no other body or file re-reads it | New |
| 5 | body | the body key (own text plus reads) is equal, so stored diagnostics are replayed | New |
| 6 | `file_report` | the assembled, tsc-ordered list is equal | New cutoff |
| 7 | `semantic_model` | the model hash is equal, so emit and IR do not rerun | New |

Typical edits and what each reruns after the plan:

| Edit | Reruns |
| --- | --- |
| Add a comment or reformat a line | parse and header only |
| Change a statement inside a function body | that body and the bodies that read its inferred return, then the file report |
| Change a function's parameter or return annotation | that signature, the bodies that read it, importers that read its `decl_shape` |
| Add or remove an export | `export_names` readers and units that import by name or by `*` |
| Change one compiler option | only units whose read log contains that option |
| Edit a script (global) file | provider query, then units keyed by the provider digest; the global second pass is the first to become a query |
| Edit `tsconfig.json` or add or remove a file | the graph and resolution epoch, then units whose resolution changed |

Guarantees that keep incremental results trustworthy:

1. **Equal to cold.** For any sequence of edits, the incremental report equals the cold report on the final text, byte for byte. A test replays recorded edit sequences on corpus files and runs the per-body shadow mode (both paths, compare, promote at zero differences).
2. **Reads are recorded, never guessed.** A query that reads anything outside its arguments is a bug. The shadow mode is what catches an incomplete read set.
3. **Cancellation is clean.** A stopped query stores nothing; finished memos survive.
4. **Bounded memory.** The LRU is extended to the per-item memo and the model, sized from measurements on a large project.
5. **Persistent.** Arena-free memos (shapes, models, library snapshots) go to the disk cache by hash.

The measurement that decides whether body-level incrementality ships: median time from an edit inside a function body to updated diagnostics on the same project, against today's file-grain rerun. Without a win there, file grain stays.

## Type-checking engines

The first draft organized the checker (registries, files, queries) but not the type-checking logic itself. This section names the engines, gives each one owner and one contract, and keeps what already works: the per-unit arena, candidate-based inference and the portable boundary.

### Phases of a unit check

`bodies_phase` already runs close to rustc's order. Naming the phases lets passes and rules register into the right one.

| Phase | rustc analogue | What it does | Today |
| --- | --- | --- | --- |
| bind | HIR and name resolution | Oxc parse, semantic model, `oxc_cfg`; build the flow table per body | `parse.rs`, `flow.rs` |
| collect | `type_of`, `fn_sig`, `generics_of` | Resolve item signatures into portable `ItemSig` (type parameters, defaults, constraints, parameters, declared return) | `declare_phase` |
| wf | well-formedness checks | Duplicates, heritage, constraint satisfaction on type arguments, overrides, class `implements` | scattered passes |
| typeck | `typeck(body)` | Walk statements and expressions of one body, record typed facts | `statements/*`, `expressions/*` |
| analyses | MIR passes | Reachability, definite assignment, unused, over the flow graph | `reachability.rs`, `unreachable_code.rs` |
| publish | diagnostic emission | Order, dedupe, tsc ordering | `publish` |

### Relations

One entry point decides whether one type relates to another, and the answer keeps its reason.

- `relate(source, target, mode)` returns `Related`, `NotRelated(reason)` or `TooComplex`. Modes: identity, subtype, assignable, comparable. Subtype and assignable are different relations and share a walker, not a cache entry.
- All relation state (the pair cache, the pairs in progress, the depth and size counters, the overflow flag) lives in one session passed explicitly, so no thread-local state decides an answer. `RelationCache` and `SemanticQueries` in `tsox-types` are the starting point.
- `TooComplex` is never turned into a plain `false`. The caller chooses: a conservative accept, or a named partial result, according to the contract table below.
- `NotRelated` carries the first failing member path, so the elaboration under TS2322 and friends comes from the decision, not from re-running the comparison.

### Generics and inference

TypeScript infers from candidates ranked by priority, not by unifying type variables, so tsox keeps `GenericInferenceSession` (candidates, priorities, locked parameters, `finish`) and does not copy rustc's inference table. What it takes from rustc is the discipline around speculation.

- **Probes.** `ctx.probe(|c| ...)` snapshots the length of the diagnostic list, the contextual type, the narrowing state, pending flow notes, inferred-return collection and the relation session, and rolls all of them back unless the probe commits. Overload resolution runs each candidate in a probe. This replaces rollback by truncating diagnostics, which cannot undo anything else a candidate touched.
- **Structured candidate failures.** A failed candidate returns `CandidateFailure { candidate, arg_index, kind }` with kinds such as arity mismatch, constraint violation, argument not assignable, inference failed. The reporter for TS2769 and TS2345 reads the failures; it never parses a truncated diagnostic list.
- **Contextual types as guards.** The contextual type (a field on the context today) is set and restored through an RAII guard, so an early return cannot leave a stale expectation.
- **Generics as portable data.** `ItemSig` carries type parameters with defaults, constraints and variance annotations, so a caller in another unit instantiates a callee without the callee's arena.

### Member resolution

Two levels, so structure and policy never mix.

1. **Structural lookup** in `tsox-types`: given a type and a name, return `Found(member)`, `Missing` or `Incomplete(reason)` (unresolved reference, circularity, depth). It handles unions, intersections, mapped and conditional results, index signatures, apparent types and `this` types.
2. **Access policy** in the checker: readonly, accessibility, optional, `noUncheckedIndexedAccess`, `exactOptionalPropertyTypes`. The policy layer chooses TS2339, TS2551, TS2576 and the related codes from the structured result. `Incomplete` produces no error and bumps a counter.

### Narrowing

- **Rules dispatch on the syntactic form of the test**, so they do not compete: `typeof x === "k"`, `x instanceof C`, `"k" in x`, `x.k === literal` (discriminant), `x === literal`, a type-predicate call, truthiness, and optional-chain containment. The top of one test expression matches exactly one form.
- **Combinators are not rules.** `&&`, `||`, `!`, parentheses, comma and `??` become flow edges and conditions in the flow builder. A test like `x?.kind === "a"` is the optional-chain split plus a discriminant rule, which is why first-match at the top of the expression is safe.
- **One implementation.** The forward overlay and the graph resolver call the same rules, each with its own fixtures. Today the resolver reuses the overlay's helpers, so agreement between the two proves nothing about tsc; one rule set removes that risk by making the rules themselves the tested unit.
- **One reference identity.** A `Place` (a symbol plus a property path with literal keys) is used by narrowing, assignment targets and definite assignment. It extends the assignment-target path the flow table already has.

### Diagnostic provenance

A semantic decision becomes a `DiagnosticCause`; a reporter turns the cause into a code, a message and a span. Related information (the elaboration chain, the failing candidate) is built from the cause. Messages stay in `diagnostic_messages.rs`. Each code has an owner entry (module and fixture directory), so the gates can say which module a failing code belongs to.

### One contract per operation

Every engine answers with the same four outcomes. None is ever collapsed into another.

| Operation | Result | Notes |
| --- | --- | --- |
| `relate` | Related, NotRelated(reason), TooComplex | TooComplex never becomes `false` |
| member lookup | Found, Missing, Incomplete(reason) | Incomplete prints nothing |
| call resolution | Resolved(signature), Failed(candidate failures), Unsupported(feature) | probes isolate candidates |
| narrowing rule | Refined(type), NotApplicable | NotApplicable tries the next form |
| flow read | Known(type), Unreachable, Unknown(reason) | Unknown falls back and is counted |
| import lookup | Found(shape), Missing, Incomplete | feeds the read log |
| any query | plus Cancelled | stores nothing |

Unknown and Unsupported never print a diagnostic. Each bumps a named counter shown by a stats flag, so a fallback is a number on a dashboard, not a silent branch.

## Flow architecture

Today three mechanisms decide reachability, assignment and narrowing (see where the code stands). The plan keeps the graph both sides already build, makes the resolver the single owner of narrowing reads, and moves the rest onto the same graph only after shadow comparison proves equality.

### The answer type and the events

The resolver answers `Known(type) | Unreachable | Unknown(reason)` instead of the current type-or-bottom-or-unknown. Each fallback today has a reason: exception edge, nullish operand, compound assignment, destructuring, outer-function variable, assertion function in the file, write later in a loop body. The reason is a counter, and a fallback is removed when its counter reaches zero on the corpus.

Flow events beyond today's assignment, condition and switch clause:

| Event | Source | Used for |
| --- | --- | --- |
| `Call` | an expression statement whose callee is a dotted name | assertion functions, never-returning calls |
| optional-chain split | `a?.b` | narrowing of `a` on both paths |
| `ReduceLabel` | after a `finally` block | normal exit ignores exception and return paths |
| destructure, property write | patterns, `a.b = x` | narrowing after writes |
| declare uninitialized | `let x: T;` | definite assignment |
| return, throw | explicit and implicit | return-path checks |

### Algorithms

- **`catch` entry.** tsc adds the flow before a `try` and every assignment and call node inside it as antecedents of the exception label, and the `catch` begins there. `oxc_cfg` already draws an error edge from each block in a `try` to the handler, but tsox's graph gives the `catch` one antecedent and the resolver gives up on it. The plan assigns each assignment and call node, by AST range, to the innermost enclosing handler, and removes the early bail-out so the label is an ordinary join.
- **`finally`.** The finally label joins every normal exit and every exception path; oxc's `Finalize` edges cover return, break and continue. The node after the block is a `ReduceLabel` whose target is the label and whose only antecedents are the normal exits, so normal continuation does not inherit exception paths.
- **Calls.** Only dotted-name call statements become nodes. The checker supplies an `EffectsOracle`: `Never`, `AssertsTruthy(arg)`, `AssertsType(arg, ty, positive)`. As in tsc, the callee needs an explicit type annotation to qualify, which avoids circular inference. Once this works the "whole file falls back if it declares an assertion function" rule goes away.
- **Optional chains and `??`.** The continue path carries "object is not nullish", the skip path carries "object is nullish", and a join closes the chain. The nullish conditions exist; what is added is narrowing by them (today they return Unknown).
- **Loops.** The head is solved by repeating until the type stops changing. Each round only adds constituents of the declared type, so a loop settles in at most one round per constituent plus one. If it does not, the answer is the declared type, a wider but sound result, instead of whatever the last round gave.

### Ownership

| Fact | Owner after the plan | Condition for moving |
| --- | --- | --- |
| Narrowing reads | graph resolver | each fallback counter at zero on the corpus |
| Dead code, return paths | `reachability.rs` stays | graph verdicts equal its verdicts on the corpus in shadow mode; then derive them from the graph with `is_infinite_loop_start` and a tsc constant-condition closure |
| Definite assignment (TS2454) | a separate lattice over the same graph | classify the current mismatches first |

Before building definite assignment on the graph, classify the TS2454 results: the type-correction proposal's corpus table shows 481 of 587 as mismatches, not gaps, so the problem may be the reported code or span and not the analysis. A different lattice (assigned, maybe assigned, unassigned) over the same graph is the plan only if analysis turns out to be the cause.

### Cutover protocol

Every flow change ships in shadow first: run the old path and the new path, compare per read, count differences by reason, and promote one reason at a time. A fixture exists for each reason. The graph never replaces the overlay for a construct whose fixture is missing.

## The semantic model

Emit, the language server and any IR need the same answer to "what is the type and symbol of this node?". Today that answer exists only while a unit is being checked. The model keeps it, as data a consumer can read without a checker.

| Table | Key | Value | Used by |
| --- | --- | --- | --- |
| `node_type` | node id (offset-free) | portable type | hover, IR lowering, enum emit |
| `node_symbol` | identifier node id | `DeclKey` | go to definition, import elision, rename |
| `flow_facts` | node id of a reference | narrowed type at that point | hover, IR (known type per use) |
| `const_values` | declaration key | literal or enum value | `const enum` inlining, constant folding |
| `module_info` | file | exports, import uses (value or type only) | import elision, bundlers |
| `diagnostics` | file | tsc-ordered list | CLI, LSP, build gate |

Design rules:

- **Written during checking, by one writer.** One `insert` at the point a type is assigned. For `flow_facts` this needs one owner of narrowing: the resolver answers pending reads after the file is checked, so the model records at resolution time, and the overlay stops writing once a construct moves to the resolver. Two writers would disagree.
- **Node keys must be unique.** A key derived only from a subtree hash gives two identical expressions in different places the same key. The key includes the parent path or sibling index, or the `DefKey` plus the index within the item. This needs a check against `keys.rs`, which this revision did not verify.
- **Portable, opt-in, offset-free, versioned.** As in the first draft: arena-free types, switched on by emit, LSP and IR callers so the cold CLI does not pay, keys stable under text moves, and a format version in the cache key.

## Rule modules and registries

The repair framework already works this way: a `Rule` is data produced by a function chosen by the parser message. The rest of the checker follows the same pattern, as plain tables of function pointers, so a table reads top to bottom.

| Registry | Entry takes | Entry returns | Today lives in |
| --- | --- | --- | --- |
| `REPAIRS` | parser message, text, span | an optional `Rule` | `syntax_repair.rs` |
| `NARROWERS` | a test expression of one syntactic form, base type, sense | refined type or nothing | `narrow.rs` (2,306 lines) |
| `CALL_FORMS` | a call or `new` expression and its context | resolved signature and argument checks | `expressions/calls.rs` (1,912 lines) |
| `PASSES` | the context and a scope (file or body) | diagnostics for the codes it owns | nine passes called by hand in `bodies_phase` |
| `EFFECTS` | a call node | `Never`, `AssertsTruthy`, `AssertsType` | not present |

Every entry has a name, the diagnostic codes it may emit, a one-line doc, and optionally the first TypeScript version it models. A test asserts that each entry has all of these. The harness prints "TS2300 → `duplicates`" from the tables and fails when a code in the corpus has no owner, which is the contributor map. Order and precedence are table order. Each registry is introduced as a pure refactor and verified by an identical full-corpus diff before any new rule is added.

## Contributor model

A contributor adds one behavior by touching one directory and one test file. Three rules make that true.

1. **One feature, one module.** Split the large bridge files by feature: `narrow/` (typeof, in, instanceof, discriminant, equality, predicate, truthiness), `calls/` (resolve, infer, overload, spread), `exports/`, `declare/`. Each module has a header saying what it owns and what it must not touch. The split is mechanical and changes no behavior.
2. **Rules register, they are not wired by hand.** Adding one is a new file plus one table line.
3. **Layers are enforced.** An xtask `check-layers` command fails CI when a crate imports across the allowed edges: `tsox-base` depends on nothing in the workspace; `tsox-types` on base only and never on Oxc AST types; `tsox-checker` on base, types and Oxc; `tsox-db` and `tsox-project` on checker and types, and only they use Salsa; `tsox-emit`, `tsox-lsp`, `tsox-cli` and `tsox-wasm` on the semantic model and the database, never on `bridge` internals.

| To do this | Touch | Test with |
| --- | --- | --- |
| Narrow by a new guard | one file in `narrow/` and the `NARROWERS` line | a fixture compared with tsc |
| Model a new assertion or never-returning call | one `EFFECTS` entry | a fixture with the call |
| Fix a false positive | the module that owns the construct | the corpus files from the map, not the whole corpus |
| Add a diagnostic | `diagnostic_codes.rs`, `diagnostic_messages.rs`, the check, and its owner entry | message text from tsc 6.0.3 |
| Repair a syntax shape | one `RepairRule` | a repair test with expected note and offset map |
| Support a new library feature | the lib loader table | a lib-version fixture |

Supporting tooling: `cargo xtask corpus` now does what this step planned (fetch by tag, a stored map, verify; see docs/corpus.md); the planned `snapshot`, `fetch` and `check` commands (stored tsc diagnostics, on-demand download of selected corpus files, regression check) belong in `tsox-xtask` before the project is opened up. A scaffold command creates a rule or pass file, its table line and an empty fixture.

## The host boundary

Everything that touches the outside world goes through one trait, so the same checker runs in a terminal, an editor, a browser worker and a test with no disk. The LSP's `Host` trait in `navigate.rs` is generalized and moved down, not invented.

| Capability | Today | Behind the trait |
| --- | --- | --- |
| Read a file's text | `std::fs` in `tsox-db/project.rs` | `read(path)` |
| List and resolve modules | `oxc_resolver` and `std::fs` across `tsox-project/module_resolution/*` | `resolve(from, specifier)` |
| Read `tsconfig.json` and lib files | `std::fs` and `standard_lib.rs` in `tsox-base` | `read` plus an embedded lib bundle |
| Disk cache | `tsox-cli/cache_dir.rs` | `cache_get(key)` and `cache_put(key, bytes)`, optional |
| Threads | `rayon` behind a feature | a `Spawner` the host provides, or none |
| Time and cancellation | `CancelToken` | unchanged |

Only `tsox-cli` and a native host crate may call `std::fs`; `check-layers` rejects it elsewhere. The default host is in memory, which is also the test double and the WASM default. Library files ship as an embedded, hash-keyed bundle. With no spawner the scheduler runs units one after another; the output is identical because publication order is already fixed.

## The TypeScript profile

The vendored libraries are TypeScript 6.0.3 (`TYPESCRIPT_LIB_VERSION`), and checks cite tsc behavior in comments, but the version is not part of the check options. Add a profile with the compiler version, separate from the ECMAScript target. A registry entry may declare the first version it models, and a version read lands in the typed read log like an option, so a version change reruns only what consulted it. The oracle for gates is a pinned tsc of the profile's version.

## Quality gates

| Layer | What it proves | Run by |
| --- | --- | --- |
| Unit tests per rule | one rule behaves on small inputs | `cargo test`, every change |
| tsc-verified fixtures | one file per case, message and code match tsc 6.0.3 | `compare-local.sh` on the author's files |
| Affected-corpus run | files the change can touch did not regress | `cargo xtask corpus run` then `verify` |
| Conformance by code family | the codes a change targets improve and the others do not regress | corpus compare, grouped by owner |
| Fallback counters | every Unknown(reason) trends to zero, never up | stats flag on the corpus run |
| Equal-to-cold replay | incremental reports equal cold reports after edit sequences | recorded edits on corpus files |
| Per-body shadow | memo path and full path produce identical diagnostics | corpus run, promote at zero differences |
| Determinism | same input, same bytes, regardless of threads | existing test in `tsox-cli/tests`, extended to the model |
| Time budget | no file above 5000 ms | the compare run reports per-file time |
| Full corpus | global changes only (arena, union, resolution) | scheduled run, parts 07, 09 and 15 timed apart |

## Roadmap: emit, WASM, IR

Each goal reads the same semantic model and needs no change to the checker once the model exists.

- **JS emit (`tsox-emit`).** Parse with Oxc, strip types, print with Oxc's code generator and source maps. Emit does not depend on the checker; type errors gate it only when asked (`noEmitOnError`). Import elision and `const enum` inlining read `semantic_model`. Whether to use Oxc's transformer or an own pass is decided when this step starts.
- **WASM.** (a) The checker in a browser or a worker needs the single `Host` trait, with `std::fs`, `rayon` and the disk cache behind features; `tsox-wasm` is 78 lines, so the work is in the lower crates. (b) WASM as an output target comes from the IR.
- **IR and native.** A typed mid-level IR lowered from the AST plus the semantic model, then a backend (Cranelift for fast native builds, WASM second). TypeScript types are not sound at run time (`any`, assertions, structural subtyping, excess properties), so the IR starts with a statically typed subset and records, for each value, whether its type is trusted. Cross-body type identity may be needed here, which is the trigger for revisiting the shared interner.

Order: semantic model first, emit second (it ships without the IR), host trait and WASM checker third, IR last on a model already exercised by two consumers.

## Migration plan

Four tracks run in parallel because the structure work and the correctness work do not conflict. Each step ships alone as a patch series and starts only when the previous step in its track passes its gate. Step 0 comes first because it decides the others.

**Step 0, measure (no code).**

- Count `scc_run` entries after one body edit, one script-file edit and one option change, to rank the four recheck suspects.
- Time the declare phase against the bodies phase on a large real project, to decide whether the per-body memo can pay off.
- Add fallback counters by reason to the flow resolver and run them on the corpus.
- Classify the TS2454 mismatches: wrong code, wrong span or missing analysis.

| Track | Step | Change | Behavior change | Gate before the next step |
| --- | --- | --- | --- | --- |
| A structure | A1 | `check-layers` xtask; dependency edges written down | none | zero violations on main |
| A structure | A2 | Split `narrow.rs`, `calls.rs`, `exports.rs`, `declare.rs` by feature | none | identical diagnostics on the full corpus |
| A structure | A3 | Registries: `NARROWERS`, `PASSES`, `REPAIRS`, `CALL_FORMS`, `EFFECTS` | none | identical diagnostics; a rule is one file plus one line |
| B correctness | B1 | Flow answer type with reasons; counters | none | counters match the old fallbacks exactly |
| B correctness | B2 | `catch` entry and `finally` (`ReduceLabel`) | fewer fallbacks | shadow equal or better, fixture per construct |
| B correctness | B3 | `Call` events and `EFFECTS` | fewer fallbacks | assertion-file fallback reaches zero |
| B correctness | B4 | Optional-chain narrowing; loop bound with declared-type fallback | fewer fallbacks | counters trend to zero |
| B correctness | B5 | Relation session and `RelationDecision` | none to diagnostics | identical diagnostics; no thread-local state |
| B correctness | B6 | Probes and `CandidateFailure` | possible message changes | TS2769 and TS2345 match tsc on the family |
| B correctness | B7 | Two-level member resolver | fewer TS2339 mismatches | corpus compare by code |
| C incremental | C1 | Option reads; fix the provider and plan suspects found in step 0 | fewer unit reruns | `scc_run` counts drop; equal-to-cold |
| C incremental | C2 | Item tree, `DefKey`, typed read log (record only) | none | read sets complete on the corpus |
| C incremental | C3 | Per-body memo in shadow mode | none | zero differences; edit-in-body benchmark improves |
| C incremental | C4 | Promote the memo; inferred returns as a demand query | possible ordering differences | diagnostic order matches tsc |
| D consumers | D1 | `semantic_model` side tables, one writer | none to diagnostics | model hash deterministic; no file above 5000 ms |
| D consumers | D2 | One `Host` trait; WASM build of the checker | none | checker runs in a browser test with no `std::fs` |
| D consumers | D3 | `tsox-emit`, then the IR crate | new feature | emit output compared with tsc on a sample set |

B1 comes before any behavior change so every later flow step has a number to move. A1 to A3 and B1 to B2 are safe first contributions: moves and counters change no output, and a `catch` fixture is a self-contained task.

## Risks

- **Regressions while splitting.** A1 to A3 are moves, verified by a full-corpus diff before and after. Parts 07, 09 and 15 are timed separately because the repair loop is slow there.
- **The per-body memo costs more than it saves.** The declare pass runs every time and portable conversion has a cost. Step 0 measures it and file grain stays as the fallback.
- **Incomplete read sets.** An unrecorded read gives a stale result. The shadow mode and the equal-to-cold replay exist for this and gate promotion.
- **Diagnostic order.** tsc orders by check order, not position; per-body results must reproduce that order in `file_report`.
- **Two narrowing writers disagree.** Until each construct has one owner, the model's `flow_facts` can contradict the diagnostics. The ownership table and the counters are the guard.
- **Contributors outrun reviewers.** CODEOWNERS per crate and a decision log for every change to a layer boundary.
- **IR scope creep.** The IR stays out of the checker's critical path until emit has shipped.

## Open decisions

1. Per-unit arenas versus a shared interned type store. Decided by body-level measurements and by what IR lowering needs.
2. Whether the per-item memo is promoted to Salsa tracked queries once arena rebuild is cheap.
3. Whether `reachability.rs` is retired or kept as the tsc-accurate reference. Decided by shadow equality on the corpus.
4. Whether definite assignment moves to the graph. Decided by the TS2454 classification in step 0.
5. Oxc's transformer or an own pass for emit.
6. How the shared model's node keys stay unique under identical subtrees.
