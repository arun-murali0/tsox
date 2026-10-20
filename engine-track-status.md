# Engine Track: Status, Decisions, and Remaining Work

**Evidence base.** E0 checked on 2026-10-18 against `main` at `5789d5a`; E1 to E3 checked on 2026-10-19 against `features` at `1fe24cf`, and the full suite re-run on 2026-10-20 against `main` at `bcc757d` (`cargo test --all-features`: 1,208 passed, 0 failed, plus the corpus abort test at stride 1) (the
crate is now `tsox`; the depth guard and the 4,000-bracket limit are merged). Claims marked *(verified)* were checked in the code or by running the
binary on that tree. Claims marked *(not rechecked)* come from earlier reads and have not been
confirmed this round.

## 1. Goal

Stabilize the project engine before making further changes to the type system.

The engine is responsible for module resolution, dependency tracking, unit scheduling,
cross-file declarations, result publication, and incremental execution. These parts need
predictable behavior before changes to type representation, generic inference, and arena
allocation begin.

The objective is not to freeze the current implementation. It is to establish stable contracts
so the type system can evolve without repeatedly changing the project driver.

## 2. Current status

| Stage | Status | Remaining exit condition | Check that closes it |
|---|---|---|---|
| E0: Safety net | Complete for the corpus | Preserve full-corpus and determinism evidence | `TSOX_CORPUS_STRIDE=1 cargo test --test corpus_aborts` |
| E1: Cross-file completeness | Complete | Real-project runs of `tsox --partial` beyond the corpus; the reach columns only mean something on multi-file projects | `tsox --partial` summary (corpus: 518 partial files of 10,083, 449 of them `UnrepresentableDeclaration`) |
| E2: Graph shape and edge kinds | Complete | Typed dynamic-import tracking (deferred to E6); `ModuleTopology` decision (after E4) | `only_a_dynamic_import_is_kept_out_of_the_ordering_and_is_listed_apart`; dependents lists for dynamic imports |
| E3: SCC as one unit | Complete | The cross-file circular-return case (needs on-demand inference, later design) | `tests/scc_contract.rs`, `tests/circular_returns.rs` |
| E4: Scheduler and pipeline | In progress (phases 1 and 2 done) | Read, hash and header stage and the single parse (phase 3, needs the AST ownership design) | `tests/scheduler.rs` on random graphs; `random_projects_give_the_same_report_whatever_the_timing` for the real checker |
| E5: Project configuration and resolution | Not started | Configuration-aware resolution and package exports | A project using `paths` and package `exports` with no false TS2307 |
| E6: Library and ambient declarations | Not started | Shared provider, `declare global`, module augmentation | Merge fixtures that give the same result in any file order |
| E7: Query layer | Prototype only | Salsa integration and verified invalidation | One test per row of the LLD invalidation matrix |
| E8: Limits, safety, measurement | Partially implemented | Budgets, recursion protection, memory checks, metrics | Adversarial-input suite; metrics from LLD 9.5 |

**E0 evidence.** The whole corpus (10,083 files) and the 8 known crashers ran one file per
process with `RUST_MIN_STACK` unset and no abort *(verified on `5789d5a`)*: about 215 s as a
debug test, about 135 s with the release binary and `scripts/isolate-fixture-aborts.sh`
(measured before the rename). The env var is now `TSOX_CORPUS_STRIDE`; the old name is ignored
without a warning, and a run with it set silently covers only the default 1-in-500 sample. The determinism test
(`tests/determinism.rs`, byte-identical output across thread counts) passes in the full suite
*(verified)*.

**E0 limit.** E0 means no abort on the agreed corpus. It does not mean no input can abort: the
parser still overflows on a few shapes the bracket limit does not count (section 7), and in a
debug build oxc's semantic builder overflows on very long flat chains. That limit is tracked
under E8.

The remaining stages should be closed against explicit exit tests rather than being marked
complete merely because the main implementation exists.

## 3. Decisions

### E1: Measure partial units before extending the export model

The next E1 task is to measure incomplete units on real projects and group them by reason.

The checker already exposes partial results through `tsox --partial` *(verified)*. It prints
each partial file with its reasons, but it does not count the dependents a partial file affects.
The measurement needs that count, so a small change to the report comes first.

The measurement should identify both the original reason for incompleteness and the number of
dependent units affected. One incomplete declaration can propagate through a large dependency
graph, and counting affected files alone makes a single root cause look like dozens of
separate problems.

**Root cause or propagation.** The 15 reasons split three ways *(verified from the enum)*:

- Propagation: `StarFromPartialModule` and `NamespaceStarFromPartialModule` mean the file is
  partial only because a module it re-exports from is.
- Mixed: `UnresolvedReExport` can come from a missing module, a name the source does not
  export, or a source whose list is itself partial.
- Root causes: the other twelve (`CheckerCrashed`, `UnsupportedDeclaration`,
  `EmptyBindingPattern`, `ShorthandAmbientModule`, `AnonymousDefaultWithoutSignature`,
  `NonObjectExportAssignment`, `UnknownLocalExport`, `DeclarationFileWithoutExportStatement`,
  `UnrepresentableDeclaration`, `StarFromUnknownModule`, `NamespaceStarFromUnknownModule`,
  `CycleDidNotSettle`).

The measurement should count root causes first and report propagation separately.

**The reason with no test.** `UnresolvedReExport` is reachable, but only for erroneous
programs: `export { a } from "./a"` where `a.ts` does not export `a` (tsc's TS2305), or from a
module that does not resolve (TS2307). It needs two fixtures and should stay.
`UnsupportedDefaultExport` was dropped: with the pinned oxc 0.151 its producer, the catch-all
arm of the `export default` match, cannot be reached, so that arm now marks
`UnsupportedDeclaration`.

Declaration files without export statements also need explicit treatment. They should not
automatically be considered complete by exporting every declaration. The engine must
distinguish global declaration files, external modules, and ambient module declarations before
deciding which names are visible. Today these stay partial under
`DeclarationFileWithoutExportStatement` *(verified)*. tsc also reports TS2459 for a name a
`.d.ts` declares locally but does not export, and tsox has no such code *(not rechecked)*.

**Exit condition:** Real-project measurements are recorded, reachable partial-result states are
covered, and the remaining cases have documented semantics.

### E2: Keep dependency edges semantic

The graph should represent dependencies that affect checking, not merely every import-shaped
expression in the source.

For dynamic imports, the dependency rule is based on whether the imported module's type
information is required by the current unit. A dynamic import currently finds its file and is
checked with the rest, but does not order it *(verified by
`a_dynamic_import_finds_the_file_but_does_not_order_it` in `tests/project_imports.rs`)*. Typed `import()` and `typeof import()` need to be implemented before
the distinction can be completed.

Dependents of dynamic imports are recorded: `ModuleGraph::dynamic_dependents` lists the files
that load a file with `import()`, apart from `dependents`, so nothing that orders by `dependents`
can pick up an edge that must not order. What has to rerun when a file changes is `dependents`
together with `dynamic_dependents`. `only_a_dynamic_import_is_kept_out_of_the_ordering_and_is_listed_apart`
pins the rule for dynamic imports, side-effect imports, re-exports and import-equals. Static, type-only and augmentation edges are not in that fixture, and nothing pins one file importing the same file both ways, which lists the importer in both `dependents` and `dynamic_dependents` *(verified)*.

`ModuleTopology` is called only from tests and benches today *(verified)*. It should remain only
if it continues to provide useful graph operations or test and benchmark coverage. Otherwise,
remove it rather than keeping a second representation without a production responsibility.
Decide after E4, when the scheduler shows whether it needs the extra operations.

**Exit condition:** Dependency ordering and edge kinds have defined semantics, with typed
dynamic-import handling deferred until E6.

### E3: Keep iterative SCC settlement and amend the HLD

The current iteration converges for the tested exports and inferred returns. Replacing it with
one shared arena per SCC would require substantial changes to type storage, recursive
references, and declaration resolution.

That redesign belongs with the later type-system work, not as a prerequisite for stabilizing
the engine.

For now, an SCC remains the unit of scheduling, settlement, publication, and failure isolation.
Its internal type-storage strategy remains an implementation detail.

The iterative model must enforce the following conditions. Each now has a test, in
`tests/scc_contract.rs`, `tests/circular_returns.rs` and `tests/project_imports.rs` *(verified,
E3 patch)*.

- Each SCC settles independently. *(Done: `settle_members` groups a layer's members by
  component, each with its own bound; `two_unrelated_cycles_in_one_layer_are_settled_on_their_own`.)*
- Convergence is determined by published semantic results, not incidental internal IDs or
  execution order. *(Pinned: the outcome does not change with the order of the entries.)*
- Non-converging units produce an explicitly incomplete result. Results from the last
  unsuccessful round must not be published as authoritative. *(Done: the files of an unsettled
  cycle carry the new reason `CycleDidNotSettle`, so `--partial` lists and counts them, and their
  exports are flagged partial. Measured: the last round does not report a false missing name,
  because partial lists keep it quiet, so no diagnostics are dropped.)*
- An incomplete SCC must not cause downstream units to report misleading missing-export errors.
  *(Already held, and tested; also, an importer keeps reporting its own unrelated errors.)*
- Circular inferred-return dependencies must be detected instead of silently degrading to `any`.
  *(Done for functions declared in one file: TS7023, as `TSR1404`, is reported on each function
  on the circle, including recursion inside a larger return expression such as `n * f(n - 1)`.
  A bare `return f(...)` inside `f` is not reported, because tsc leaves it out of the return type
  (decision 14); that call still types as the placeholder `any`. A circle through an import, a
  method or a variable is not seen and still reads as `any`; the cross-file case needs on-demand
  inference, which is the later design.)*
- One non-converging cycle must not mark an unrelated cycle incomplete. *(Done, same change as the first condition.)*

The HLD now documents this as the current implementation contract and leaves a shared-arena
design as a future option. Amended: the cycle bullet and the "Revisit if" line (`HLD.md`, around
line 153), decision 4 in section 4.2 of `HLD.md`, and section 4.3 of `LLD.md`.

This avoids combining a scheduler change with a major arena rewrite. It also gives the existing
implementation a precise correctness requirement instead of treating convergence on current
fixtures as proof that every cycle is handled correctly.

**Exit condition:** Settlement, failure propagation, and circular-inference behavior are
deterministic and covered by targeted fixtures.

### E4: Replace layer barriers with dependency-driven scheduling

The current scheduler runs graph layers in parallel and waits at each layer boundary. A ready
queue should allow a unit to start as soon as all its own dependencies finish.

The queue should operate on SCCs rather than individual files, preserving E3's scheduling
contract.

The pipeline should also read, hash, and parse each file once where practical. A file is parsed
at least twice today: once in discovery for edges, and once in the checker. A declaration file
can be parsed a third time in `ambient_providers` *(verified: `discovery.rs`, `check.rs`,
`bridge/parse.rs`)*.

Single-parse execution must be designed together with AST ownership and allocator lifetimes.
Avoiding a second parse is useful only if the resulting ownership model remains clear and does
not introduce excessive memory retention.

Result publication must remain deterministic even when units finish in a different order.

The existing cancellation token should be passed through the project driver and checked at unit
boundaries. `CancelToken` exists and the checker honors it, but the project driver never passes
one *(verified)*. Worker panics are already converted into one internal-error diagnostic marked
incomplete (`CheckerCrashed`); this must stay true when the scheduler changes, with no
partially produced result appearing complete.

**Exit condition:** On randomly generated graphs, every unit executes exactly once after its
dependencies complete. Independent units can overlap, output remains deterministic, and E0
remains green. The `scc` random-graph generator can be reused.

**Status.** Phase 1 is in: `src/module_resolution/schedule.rs` runs units from a dependency count,
a unit reads only the results of the units it waits for, outcomes come back indexed by unit, a
panic is recorded and its dependents still run, the cancel token is checked as a unit starts,
and units that wait on each other are refused before anything runs. `tests/scheduler.rs` holds
the exit test on random graphs at one, two and eight threads *(verified)*.

Phase 2 is in (decision 16): `check_project` runs the components of the graph on that queue. A
file reads only what the components it imports published, plus a `Globals` value fixed before
the pass (ambient modules, augmentations, UMD namespaces and their declarations). Ambient
providers come from the parse discovery already made. `check_project_cancellable` passes a
`CancelToken` through the driver, and `ProjectReport::cancelled` says a run was stopped. A
panic in a unit becomes an internal-error report for its files, and its dependents still run.
*(verified: full suite 1,208 passed, 0 failed; corpus test at stride 1 passes; the 10,083
corpus files as one project give byte-identical output before and after, and 40 smaller projects
agree with `--partial --cycles`.)*

**Finding fixed on the way.** Every file was checked twice. The second pass was meant for
projects with UMD globals or augmentations, but its test asked whether the UMD table was empty,
and the table got an entry for every file. The second pass now runs only when the first found a
UMD alias, an augmentation, or an ambient module outside a provider file. Measured on the corpus
as one project in a release build: about 2.3 s before, about 1.0 s after (two runs each).

**Behaviour change to know about.** The double pass had been giving an unsettled cycle twice
its round budget by accident. With the bound forced very low, a file of such a cycle can now
keep an unresolved-annotation warning. It still never reports a missing name or module, and
`an_unsettled_cycle_reports_no_missing_name_among_its_own_members` now checks exactly that.
Default bounds are unaffected: a cycle gets one round per member plus a spare.

Phase 3 is the read, hash and header stage and the single parse, which waits on the AST
ownership design. The checker still parses each file itself after discovery has.

### E5: Implement project configuration and complete module resolution

The current resolver supports useful TypeScript-oriented behavior, but the project driver does
not yet interpret `tsconfig.json`. Its contents are never parsed, and `paths` and `baseUrl` are
not read; a warning says so *(verified in `bin/tsox.rs`)*.

Configuration parsing should cover inheritance, file selection, path aliases, base URLs, root
directories, type roots, explicitly selected types, and the strictness settings needed by the
checker. Package resolution must account for relevant `exports` and `types` conditions.

The resolution configuration should be represented as a project input so a configuration change
invalidates affected resolution results.

The gate that currently silences missing package imports should be removed only when resolution
produces reliable results for supported package layouts.

This stage is more than a convenience feature. If the engine resolves the wrong file or
suppresses a genuinely missing module, later type checking can be perfectly implemented and
still produce incorrect project results.

**Exit condition:** Projects using supported path aliases and package exports resolve
correctly, without false TS2307 diagnostics. Configuration changes also produce the expected
dependency-graph updates.

### E6: Add a library and ambient-declaration provider

The current global-type name list (`lib_global_types.rs`) prevents certain unknown-name
diagnostics, but it does not provide the actual semantic shapes needed for type checking.
`Promise` and `Record`, for example, appear there only as names and cannot remain placeholders
once their behavior is required by expressions and annotations. There is no `declare global`
support in `src` *(verified)*.

Load a small, explicit library subset through the same provider used for project dependencies.
Begin with the types required by the engine and current language features, including `Array`,
`Promise`, and `Record`.

Add `declare global` and module augmentation using fixtures that verify declaration merging by
stable key and independent of file order.

Typed `import()` and `typeof import()` belong here because they depend on module exports being
represented as usable types.

**Exit condition:** Library and ambient declarations merge consistently, required
dynamic-import forms resolve to correct types, and results do not depend on file processing
order.

### E7: Integrate the query layer only after the unit boundary is stable

The existing Salsa integration is a prototype: `incremental.rs` is 206 lines, with Salsa pinned
at 0.28.5 behind the `incremental` feature, one input and one demonstration query *(verified)*.
The real checking pipeline is not connected to a complete incremental query graph.

Before implementing the full query layer, verify the behavior of the pinned Salsa version:
early cutoff, query reuse, thread handles, and cancellation.

Then introduce file inputs, module-header queries, resolution and graph queries, checking-unit
queries, and declaration projections. Use the declaration reads already recorded in
`FileReport.reads` to connect semantic dependencies to incremental recomputation.

Each row of the LLD's invalidation matrix should become a test that records which queries
execute and compares the incremental result against a cold check.

The key distinction is between reusing allocated capacity and reusing semantic results. The
current session and worker reuse behavior does not itself establish incremental checking.

**Exit condition:** Body-only edits, public-signature edits, import changes, and configuration
changes invalidate the expected queries and produce results equivalent to a cold check.

### E8: Add explicit resource limits and complete the measurements

Stack overflow detection is not stack overflow prevention, and the checker's own recursion is
now guarded; what remains is described below.

**Done (merged).** A thread declares its stack size, and the
recursive walks check the remaining stack by address before going deeper
(`src/stack_guard.rs`). It covers `infer_expression_type`, `TypeNamespace::resolve`,
`resolve_ts_type` and the stable hasher. Past the budget a file gets one `TSR9102` and the
rest is left unchecked, or an export is marked incomplete, never wrong. Graph discovery and the
CLI's flat mode now run on a 64 MB stack, the size the workers have. `tests/depth_guard.rs` runs
generated nested input through the real binary. Separately, `src/source_depth.rs` counts open
`(`, `[` and `{` in one pass over the bytes and refuses a file nested past 4,000 before it is
parsed (reported as not checked); the cut-off notice `TSR9102` is a warning, not an error. On
the latest release build every bracketed shape tried, from parentheses to nested classes, is
refused at 10,000 and at 100,000 levels with no abort *(verified)*.

**Still open.**

- Statement recursion (`check_statement`, `check_block_statements`) has no guard. A
  100,000-level `else if` chain passed in a release build; a debug build was not tried.
- Three parser shapes use no counted bracket and still abort at 100,000 levels: a ternary chain
  (`a ? 1 : a ? 1 : ...`), an arrow chain (`y => y => ...`) and nested `Array<Array<...>>`
  *(verified on release)*. They pass at 10,000. The count is also not a parse, so a regular
  expression holding a quote or backtick can skew it for a line.
- The type system will need instantiation-depth limits with a TS2589-style error. None exists.
- Budgets for recursive work, instantiation, and memory are still undefined.
- Run Miri or ASan on appropriate arena and reset paths, add a compile-fail ownership check if
  the arena design supports it, audit nondeterministic hash-map iteration in output paths (clippy
  already flags three uses of `HashMap` iteration), and implement the remaining LLD metrics.

Resource protection should progress alongside E3 and E4. It should not be postponed until the
whole type system has been rewritten.

**Exit condition:** Adversarial inputs fail in a controlled manner, incomplete results are
propagated correctly, memory ownership is checked where tooling permits, and the agreed metrics
are available.

## 4. Execution order

1. **E1 measurement:** Measure partial units by reason on real projects, starting with the
   small report change for dependent counts. This informs subsequent decisions without
   requiring a redesign.
2. **E3 contract:** Keep iteration, amend the HLD, and close convergence and failure-isolation
   semantics before changing the scheduler.
3. **E4 scheduler and pipeline:** Introduce dependency-count scheduling, deterministic
   publication, cancellation, and single-parse execution.
4. **E5 project configuration:** Implement configuration-aware resolution. This is largely
   independent of the type-system redesign and removes errors caused by incomplete project
   inputs. It does not depend on E4 and can run beside it.
5. **E6 library and ambient declarations:** Establish a usable source of library types and
   complete the dynamic-import type path.
6. **E7 query layer:** Connect actual semantic checking to Salsa after unit boundaries and
   declaration publication are stable. It needs E4's per-unit scheduling and E6's library path.
7. **E8 completion:** Continue safety work throughout the track, closing the remaining budgets,
   instrumentation, and ownership checks.

The order is deliberate. E1 supplies evidence; E3 establishes the unit contract; E4 stabilizes
execution; E5 and E6 provide realistic project inputs; E7 adds semantic reuse on top of those
contracts.

## 5. Engine exit gate

The engine track is complete when every line below is true. The status column is as of the
evidence base above.

| Condition | Status | How to check |
|---|---|---|
| E0 through E8 meet their defined exit conditions | Not met (E0 only) | Section 2 |
| The full agreed corpus completes without process aborts | Met | Corpus test, stride 1 |
| Output is byte-identical across supported thread counts | Met | `tests/determinism.rs` |
| Random-graph tests: dependencies complete before consumers, each unit runs exactly once | Met | `tests/scheduler.rs`, and the random project test through the real checker |
| Cyclic modules settle correctly or produce a controlled incomplete result | Partly met | E3 fixtures |
| A body-only edit rechecks only the affected unit where the dependency model permits | Not met | Needs E7 |
| Incremental results agree with cold-check results | Not met | Needs E7 |
| Baseline timing, allocation, memory, and query-reexecution measurements are recorded | Not met | Needs E8 metrics |
| Supported configuration and declaration-loading cases pass without false missing-module or missing-name errors | Not met | Needs E5 and E6 |

Performance improvements should be measured against a recorded baseline. Correctness and
deterministic behavior remain mandatory even when an optimization is faster.

## 6. Scope boundary

This track stabilizes the engine and its interfaces. It does not require a complete rewrite of
the type arena, generic solver, or type representation.

The existing implementation should remain the baseline until a specific change is justified by
correctness requirements or measured performance.

Once the engine contracts are stable, the type-system track can address arena layout, recursive
type representation, generic inference, subtyping, relation caching, and deeper incremental
semantic reuse without repeatedly changing project orchestration.

The objective is a dependable execution engine that can support an evolving type system, not an
engine whose internal architecture must be redesigned every time the checker gains a new
capability.

## 7. Findings to carry forward

These came out of the E0 and depth-guard work. None changes a decision above; each belongs to a
stage.

**Limits that remain (E8).**

- oxc's parser has no depth limit of its own. The bracket count covers the bracketed shapes. A
  ternary chain, an arrow chain and nested generics still overflow it, at roughly 44,000 to
  63,000 levels in a release build (measured before the bracket limit, which does not change
  them). Closing that needs the count to include `?`, `=>` and `<`, or a limit from oxc.
- In a debug build, oxc's semantic builder overflows on flat chains (member, `+`, `?.`, array
  suffix) at about 50,000 levels, before the guard runs. A release build is cut cleanly at
  100,000. The depth tests are sized for this.
- The guard works by stack address, so an extreme input can be cut at a different depth in a
  debug and a release build. Output on normal code is unaffected: it was byte-identical to the
  previous binary on 2,000 corpus files with no false `TSR9102`.

**Performance (E8, not E3).**

- A chain of `else if` branches that fall through is cubic. Time to check in release:

  | Branches | Time |
  |---|---|
  | 500 | 0.27 s |
  | 1,000 | 2.0 s |
  | 2,000 | 15 s |
  | 4,000 | over 120 s |

  Chains of `return` branches and guard clauses stay fast. The cause is that each level joins
  its branch with the join of everything below it, so the joined union grows by one member per
  level, and `alloc_union` removes duplicates pairwise (O(k²) per union). A hash or id-first
  dedupe, and skipping the union when both sides are the same type, should fix it. Chains of
  this length are rare in real code.
- A chain of `class A extends B` is quadratic: 1.4 s, 5.2 s and 34 s at 5,000, 10,000 and
  20,000, on the old build as well as the new one.
- A loop that assigns a narrowed variable is walked up to four times, and nested loops multiply
  that (the code's own comment; not measured).

## 8. Decisions still needed

1. **E1:** resolved. `UnsupportedDefaultExport` was dropped; the catch-all arm marks `UnsupportedDeclaration`.
2. **E0:** whether `scripts/isolate-fixture-aborts.sh` stays or goes. The corpus test covers the
   same ground inside `cargo test`; the script is faster (135 s against 216 s) and runs on a
   release binary. If it goes, the corpus README needs its commands changed.
3. **E8:** whether the `alloc_union` fix is worth doing now or waits for the budgets work.
4. **Process:** resolved. `cargo fmt --all -- --check` failed on `main` after the rename; `727e31f` re-formatted the files, and the check passes on `c4b1aaa` *(verified)*.
