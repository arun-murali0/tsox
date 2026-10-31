# Engine Track: Status, Decisions, and Remaining Work

**Evidence base.** E0 checked on 2026-10-18 against `main` at `5789d5a`; E1 to E3 checked on 2026-10-19 against `features` at `1fe24cf`, and the full suite re-run on 2026-10-20 against `main` at `bcc757d` (`cargo test --all-features`: 1,208 passed, 0 failed, plus the corpus abort test at stride 1) (the
crate is now `tsox`; the depth guard and the 4,000-bracket limit are merged). Claims marked *(verified)* were checked in the code or by running the
binary on that tree. Claims marked *(not rechecked)* come from earlier reads and have not been
confirmed this round. The E4 phase 3 work and measurements were checked on 2026-10-20 against
`core-engine` at `9b2b92b` with the phase 3a change applied, built with rustc 1.91.1 on a
one-core machine *(verified: `scripts/ci.sh`, 65 test binaries, 0 failed)*.

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
| E1: Cross-file completeness | Complete | More real-project `--partial` runs. One is done (zustand at its default branch, 2026-10-26, no `node_modules`): 34 files, 0 cycles, 14 partial files (11 `UnrepresentableDeclaration`, 1 `DeclarationFileWithoutExportStatement`, 1 `UnresolvedReExport`, 1 passed on by `export *`), 28 unresolved imports. The project is tsc-clean, yet tsox reports 127 errors there (89 TS2345, 23 TS2307 from the missing packages, 8 TS2339, 3 TS2322, 5 others), so the remaining ones are false positives to triage, e.g. a type parameter whose constraint does not resolve loses its members (`S` in `src/react.ts`) | `tsox --partial` summary (corpus: 518 partial files of 10,083, 449 of them `UnrepresentableDeclaration`) |
| E2: Graph shape and edge kinds | Complete | Typed dynamic-import tracking (deferred to E6); `ModuleTopology` removed (decided after E4: no production consumer) | `only_a_dynamic_import_is_kept_out_of_the_ordering_and_is_listed_apart`; dependents lists for dynamic imports |
| E3: SCC as one unit | Complete | A circle through a function that is not exported (the cross-file circular-return case is reported on exported functions, `tests/project_imports.rs`) | `tests/scc_contract.rs`, `tests/circular_returns.rs` |
| E4: Scheduler and pipeline | Complete for its exit condition (phases 1, 2 and 3a done); the single parse (3b) is deferred to E7 | None for E4. Whether to parse once is decided with the E7 ownership design (section 3, E4, and section 8) | `tests/scheduler.rs` on random graphs; `random_projects_give_the_same_report_whatever_the_timing` for the real checker; `tests/source_once.rs` for read-once |
| E5: Project configuration and resolution | Complete for its exit condition, including NodeNext module format from `package.json` `type`; `typeRoots`/`types` (E6), automatic graph updates on a config change (E7) are open. The checker reads `noUncheckedIndexedAccess`, `allowUnreachableCode` and `useUnknownInCatchVariables` from the tsconfig (`CheckOptions`, `tests/check_options.rs`); a key left out keeps the old fixed behavior | See "Not done" in section 3, E5 | `tests/tsconfig_resolution.rs`, `tests/project_config.rs`, `tests/cli_project.rs` |
| E6: Library and ambient declarations | Done for slices 1, 3, 4 and 5 and for what slice 2 lists as done; open: real generic library shapes through the provider (needs generic inference, with the advanced types work), `Record`/`Promise<T>` assignability, tuple, `Map`, `Set` and `ReadonlyArray` members, a generic interface merged across files, a dotted `typeof import("x").A.b` | See the progress notes under E6 | `tests/project_globals.rs` (1), `tests/library_members.rs` (2), `tests/module_augmentation.rs` (3), `tests/import_types.rs`, `tests/async_functions.rs` (4), `tests/type_roots.rs`, `tests/reference_directives.rs` (5) |
| E7: Query layer | Implemented behind `incremental` (`IncrementalProject`); see `docs/architecture/query-layer.md` | Cold check is about 1.9 times the full driver on 1,000 files; a file added early in path order moves ids and reruns the units that see them; the global-declaration second pass is not memoized; no LSP layer or debouncing | `tests/incremental_project.rs` (29: one per LLD 6.5 row, buffers, cancellation, ambient providers, and random content, graph and provider edits against a cold run); the Salsa spike in `src/incremental.rs` (8) |
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

`ModuleTopology` was called only from tests and benches *(verified)*, and the scheduler orders by
`ModuleGraph::sccs`, so it was removed together with the `petgraph` dependency.

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

Phase 3 is split in two, because the two halves differ in cost and in risk.

**Phase 3a (done): read and hash each file once.** Discovery already fingerprinted and hashed
every file (`FileState`, `content_hash`). It now also keeps the text it read, for a scanned
file whose bytes are valid UTF-8, and `check_file` uses that text instead of reading the file
again (`ModuleGraph::source`, `load_text`). A file with no kept text is read by the checker as
before, so a file that is not valid UTF-8 still fails to read. Two things follow. The checker
checks the text discovery saw, not whatever is on the disk later, so a file edited after the
walk is still reported as it was seen, and `changed_files` is what says it has moved. And every
scanned file's text now stays in memory for the life of the graph (text only, no ASTs).
*(verified: `tests/source_once.rs`, 5 tests; the full suite passes.)*

**Phase 3b (deferred): parse each file once.** The checker still parses each file itself after
discovery has. Measured on the 10,083-file corpus as one project, release build, one thread
*(verified, 2026-10-20)*:

| Step | Time |
|---|---|
| Discovery scan (parse and import walk) | 0.111 s |
| Checker parse only | 0.089 s |
| Checker semantic analysis | 0.072 s |
| `ModuleGraph::build` | 0.20 to 0.22 s |
| `check_project` | 0.96 to 0.98 s |
| Whole pipeline | about 1.18 s |

The checker's repeat parse is about 0.089 s of 1.18 s, roughly 7.5 %, which is the most that
parsing once could save. Keeping every discovery AST alive until the checker uses it holds 73 MB
of AST data (227 MB of allocator capacity, since each file has its own allocator) and raises
peak resident memory from about 70 MB to about 139 MB. The ASTs are about 11 times the size of
the source text, so the cost grows with project size. About 1,100 of the files have parse
errors, so the analysis time covers the rest.

**Decision.** E4's exit condition does not need 3b, and the gain is small against the memory
and the ownership design it forces. 3b is deferred and decided together with E7, whose query
layer has to settle who owns parsed trees and arenas anyway. If that design keeps parsed trees,
a single parse may follow at little extra cost. If it does not, 3b stays unneeded.

**Not measured.** The measurement machine had one core. The checker runs on all cores and
discovery runs on one thread, so on a multi-core machine discovery could be a much larger share
of the wall-clock time than it is here. Parsing once would not change that, because it removes
the checker's parse and not discovery's. Parallelising discovery is the more likely gain, and it
needs a multi-core run to confirm. *(not measured)*

### E5: Implement project configuration and complete module resolution

The project driver now reads the project's `tsconfig.json` and applies it to file selection
and to resolution.

**Done.**
- **Configuration (`config.rs`).** `ProjectConfig::load` reads a config as JSON with comments and
  trailing commas, follows `extends` (a path, a package, or a list), merges `compilerOptions` key
  by key with the extending file winning, and resolves `include`, `exclude` and `files` relative
  to the file that wrote them. With no `include` or `files` everything under the config is
  selected; `node_modules`, dot-directories and the `outDir` are left out by default; a `.ts`
  file hides the `.tsx` and `.d.ts` of the same name. A config that cannot be read, or an
  `extends` that names nothing or loops, is an error and not an empty project.
- **Resolution.** `ModuleResolver::with_tsconfig` hands the config to oxc_resolver, which applies
  `paths` (with and without `baseUrl`), `baseUrl` alone, `extends` chains, project `references`
  and `rootDirs`. A specifier the config does not map resolves as before, and an alias that
  points at nothing is still unresolved.
- **`exports` conditions follow the importing file.** An `import` statement reads the `import`
  condition; `import x = require()` and any import in a `.cts` file read `require`; a `.mts`
  file reads `import` (`ResolveMode`).
- **The config is a graph input.** `ModuleGraph::with_config` records every config file that was
  read, and `config_changed()` says one has changed or gone.
- **The gate follows the effective config.** With the config attached, `ModuleGraph::
  config_hides_bare_specifiers` decides after `extends`: only `typeRoots` can still hide a
  missing package, plus `paths` or `baseUrl` when the resolver was not given the config.
  `rootDirs` does not, because it only merges directories for relative imports. A missing
  package that nothing answers is now a TS2307 even in a project with path aliases.
- **NodeNext follows the nearest `package.json`.** When `moduleResolution` is `node16` or
  `nodenext` (or it is unset and `module` is), `ProjectConfig::uses_node_next` is true and
  the CLI builds the resolver with `with_node_next`. Then a `.ts`, `.tsx`, `.js` or `.jsx`
  file reads the `require` condition unless the nearest `package.json` says
  `"type": "module"`. `.mts`/`.mjs` and `.cts`/`.cjs` keep their extension rule, a dynamic
  `import()` always reads `import`, and `bundler`, `node` and unset options ignore the
  package type. The nearest-package lookup is cached per directory.
- **CLI.** `--project` loads the config, checks the files it selects, uses its directory as the
  root, accepts a bare file name such as `tsconfig.json`, and falls back to every file with a
  warning when the config cannot be read.

*(verified on 2026-10-21: `tests/tsconfig_resolution.rs`, `tests/project_config.rs` and
`tests/cli_project.rs` pass with the rest of the suite; the NodeNext tests are in
`tests/project_config.rs`.)*

**Not done.**
- `typeRoots` and `types` are not applied, so a config that sets `typeRoots` still keeps the gate
  shut. Both feed ambient declarations, which E6 provides.
- `exports` conditions read `moduleResolution`/`module` only to switch NodeNext on. Other
  `moduleResolution` effects are not modelled: the `node10`/`classic` rule sets, extension
  rules for relative imports under NodeNext, and `customConditions`.
- A config change is detected (`config_changed`) but does not update the graph by itself: the
  caller rebuilds. Automatic updates belong to E7.
- The compiler options the checker needs (strictness and others) are kept as written but the
  checker does not read them from the config.
- Without a parsed config, the gate falls back to searching the config text, and there it still
  counts `rootDirs`, `extends`, `typeRoots` and, when the resolver was not given the config,
  `paths` and `baseUrl`. This is deliberately cautious.

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

**Progress (slice 1 done):** a top-level declaration of a script file and a member of
`declare global { }` is now visible to every file of the project, with real types. Names are
published per file; the project table is built in path order, so the result does not depend
on the order files are listed or checked in (tested by reversing the entry list); a name
declared in several files merges like a module augmentation; a file binds only the globals it
leaves unresolved; a name the library provides is left alone until slice 2. A script lists
only its names in the first pass and is checked again for its declarations only when another
file uses one. Evidence: the repro of a script plus a `declare global` block used from a
third file goes from 5 false TS2304 errors to 0 and matches `tsc`; wrong uses are still
reported with `tsc`'s codes; 1279 tests pass; the stride-1 corpus abort test passes.
Corpus as one project against `before-e6`: diagnostics 25,670 to 25,246, failures unchanged at
1,103. **Not measured:** whether those 424 fewer diagnostics are all false errors; compare
with `tsc` before relying on it. **Known cost:** a project whose scripts use each other's
names gets a full second pass (corpus: check time about +57%, peak memory 26.7 to 79.5 MB);
narrowing it to the files that use a global and their importers is open. **Known limits:** a
file that declares a global itself does not see other files' declarations of the same name;
a global that refers to another global is typed from the first pass; `.d.ts` providers are
checked before globals exist; global namespaces are skipped; merged interfaces print as an
anonymous shape.

**Progress (slice 2, partly done):** an array has its library methods and a Promise has `then`,
`catch` and `finally`. Before, an array only had `length`, so `arr.map(f)`, `arr.push(x)` or
`p.then(f)` was a false "Property does not exist". A method that only moves or reports
elements is typed from the element type (`push`, `slice`, `indexOf`, `join`, `pop`, ...), so
wrong uses are still rejected with `tsc`'s codes; a method whose real signature needs generic
inference (`map`, `reduce`, `flatMap`, `find`, `filter`, `flat`, and the Promise chain) takes
callbacks and returns `any`, so it loses errors and invents none. Evidence: 6 tests in
`tests/library_members.rs`; 1285 tests pass; the stride-1 corpus abort test passes; on zustand
(0 errors under `tsc` and `tsgo`) tsox errors fall 104 to 94 and TS2339 18 to 8, all removed
errors being false; corpus diagnostics 25,246 to 24,761 with failures unchanged and no change in
allocations or peak memory. **Not done:** the slice's stated goal, loading a library subset
through the provider so `Array`, `Promise` and `Record` have real generic shapes: it needs the
generic inference the checker does not have yet, and loose typing is the safe stand-in until
then. `Record` mismatches and `Promise<T>` assignability are still not checked, a Promise chain
after the first call is `any`, and `ReadonlyArray`, tuples, `Map` and `Set` members are not
added. The remaining zustand TS2339 are generic-constraint members (`S extends ...`) and a
union with a Promise, not library shapes.

**Progress (slice 3, done for what is listed):** a `declare module "x" { ... }` in a module adds
to what `x` exports, and this was already in place; slice 3 tests it and closes one hole. Tests
(`tests/module_augmentation.rs`, 6) check: two files augmenting one interface both contribute and
wrong uses get `tsc`'s codes; the result is the same when the file list is reversed; an
augmentation can add a function, a constant or a new interface; and a package keeps its own
members next to an augmentation. The hole: when the augmented module's own exports are not known
in full (a package file whose `package.json` has no `name` and `version`, so it has no module
key), an augmentation of an interface or class used to stand in for the whole declaration, and
every member the package really declares was reported as missing (a false TS2353). Now such an
augmentation is used only when the module's list already has the name or is known in full; the
name stays unresolved, silently. A function or constant it adds stands alone. Evidence: 1291
tests pass; the stride-1 corpus abort test passes; the regression test fails without the fix.
**Not done:** an augmentation that merges an interface into a class (the class is reported as a
shape not yet checked, and members added this way are not checked); augmenting a namespace or
an enum; an augmentation of a module that is only declared ambiently (`declare module "x"` in a
script), which is a provider and not an augmentation.

**Progress (slice 4, done for what is listed):** `typeof import("x")` is the module object, `import("x").Name` is the
named type, and `import("x")` is a Promise of the module object, so `await import("x")` gives it. The module
object is built the way `import * as ns` builds one and only when the module's exports are known in full. A
lazy `import("x")` is read when the check can wait for `x` without making a cycle: `ModuleGraph::ordered_dynamic`
takes the dynamic edges in path order (not file-id order) and keeps one only if its target does not already
reach the importer, and the check waits for those units, so the answer is the same in every run. `dependencies`
and `dependents` are unchanged. A lazy import that would close a cycle stays unread, and of two files that load
each other the first by path is kept. Evidence: 9 tests in `tests/import_types.rs`, including the reversed
listing order; the lazy-load probe matches `tsc` on all 3 errors; 1312 tests pass; the stride-1 corpus abort test
passes; corpus diagnostics 24,761 to 24,720 with failures unchanged, allocations +0.15% and peak memory +0.6%.
**Found on the way, fixed here:** an `async function` declaration with no return annotation was never declared and
its body was never checked, so errors inside it went unreported and a call to it was unknown. It is now declared
like a sync function and returns a Promise of what the body returns (`tests/async_functions.rs`, 5 tests; zustand
errors unchanged at 94 and warnings 401 to 327). Unannotated async function expressions in a default export,
overload signatures and generators are not covered. **Not done:** type arguments on `import()` types, a
qualified `typeof import("x").y`, and a dynamic specifier that is not a plain string (all `Promise<any>` or
unresolved). **Observed once, not explained:** in one of three full-suite runs `depth_guard`'s
`a_very_long_alias_chain_is_cut_off_and_reported_once` got no nesting diagnostic; it passed alone three times and
in the other full runs.

**Progress (slice 5, and the second pass):** `types` and `typeRoots` are applied. The declaration
files `types` names are entries of the graph, found in `typeRoots` first and then as ordinary packages
(a package subpath goes through `exports`); unset, `types` takes nothing, because TypeScript 6.0 changed
the default to `[]` (tsc 6.0.3 was checked on seven cases and tsox matches all of them). With them
loaded, an `@types` directory or a `typeRoots` setting no longer holds back the report of a missing
package. `/// <reference path="..." />` and `/// <reference types="..." />` at the top of a file are now
edges, so the files they name are read and their globals count (matching tsc on a package that
references a sibling file and another package; a directive after the first statement is ordinary
comment). The second pass now checks again only the files that can change: users of a global name or a
UMD alias, files that import a specifier an ambient module or augmentation declares, files that read
from an augmented module, and everything that reads from those; a wildcard module (`declare module
"*.css"`) still checks every file; unit tests compare the narrowed result with a forced full pass.
Evidence: 1347 tests pass; `cargo check` passes under `-D warnings` with and without default features.
**Cost, measured:** corpus as one project, check step, against E5: wall 1,139 to 1,596 ms (+40%), CPU
1,116 to 1,567 ms, allocated 1,322 to 1,977 MB, peak memory 26.7 to 79.8 MB. The narrowing took about
12% off slice 4's wall time but changed neither allocations nor peak memory, so on the corpus it still
behaves like a full second pass; the cause is not yet confirmed (the likely one is a wildcard module or
a near-total set of users in the corpus). The corpus as one project is a stress case, not a real
project. **Not done:** `/// <reference lib>` and `no-default-lib` are ignored; an unresolvable
directive is a warning where tsc reports TS2688; the stride-1 corpus abort test has not been run on
this change.

**Progress (E6 close-out):** type arguments on `import()` types (`import("x").Box<number>`) and a
qualified `typeof import("x").y` resolve; the type-reference reader is shared
(`resolve_named_reference`). An unresolved `/// <reference types>` is TS2688 and an unresolved
`/// <reference path>` is TS6053, judged like an import (only when the graph is sure); `lib` and
`no-default-lib` are accepted and ignored, because the checker's library is fixed. A file that
declares a global interface itself now joins the other files' parts of it (script files and
`declare global`, either listing order); two files declaring one global name trigger the second pass
for both. In module augmentation, an interface merged into a class reaches `new C()`, an enum
augmentation keeps the original members and adds the new ones, and a namespace augmentation adds
values and types. Checked against tsc 6.0.3 on probes: same error codes. Evidence: 1366 tests pass
under `-D warnings`; narrowed and forced-full second passes still agree. **Not done:** a generic
interface merged across files keeps only the first declaration's shape; merged interfaces print as
an anonymous shape in messages; `typeof import("x").A.b` (dotted) stays unresolved; a dynamic
specifier that is not a plain string; the stride-1 corpus abort test was not run on this change.

### E7: Integrate the query layer only after the unit boundary is stable

The existing Salsa integration is a prototype: `incremental.rs` is 206 lines, with Salsa pinned
at 0.28.5 behind the `incremental` feature, one input and one demonstration query *(verified)*.
The real checking pipeline is not connected to a complete incremental query graph.

**Baseline (measured, before any E7 code).** Salsa 0.28.5 behaviors the design may rely on are
pinned by tests in `src/incremental.rs` (`cargo test --features incremental --lib incremental`, 8
pass): (1) early cutoff: a query whose result did not change does not rerun its dependents, even
though the input changed; (2) editing one file reruns only the queries that read it; (3) a database
clone is the per-thread handle, and one query runs once across 8 threads; (4) a write cancels a
running query (`Cancelled::PendingWrite`) and the setter waits for it to unwind, and a
`CancellationToken` cancels without a write (`Cancelled::Local`); a query must poll
`unwind_if_revision_cancelled()` for either to take effect.

What an edit costs today (no reuse: every edit re-checks the whole project), release build at
opt-level 1 without LTO, 2 cores, 1,000 generated files with 3 imports each: full check 0.10-0.11 s,
13.7 MB peak; after a one-line comment edit 0.12 s (the same). zustand (31 files): 0.04 s warm. This
is the number E7 has to beat on a large project; small projects will not show a gain, because the
whole check is already cheaper than a query graph's bookkeeping. Measure again on a 10,000-file
project and on the corpus before judging.

**The plan against the code.** The two-plane split matches `check_unit` / `UnitOutput`, which are
Salsa-free. Differences to settle in the design step: Salsa does not dispatch work to the tsox pool;
parallelism is one database clone per thread, so the scheduler (`run_units`) and Salsa must agree on
who owns the order; the plan's global static library arena does not exist (each file's arena is
built per unit, and the library is a name list); cancellation and LRU eviction are built into Salsa
(`Cancelled`, `#[salsa::tracked(lru = N)]`) and need no hand-made flag; there is no LSP layer yet,
so debouncing has no home. The three epochs (resolution, global scope, per-unit) map to a
high-durability input for config, an input for the ambient declaration set, and per-file inputs.

**Implemented.** The design is `docs/architecture/e7-system-design-plan.txt` and LLD 6.2 to 6.7;
what was built and where it differs is in `docs/architecture/query-layer.md`. Inputs are per-file
`SourceFile`s and one `ProjectInput` (graph and epoch). Queries: `module_header`, `check_scc` (one
component, LRU-bounded), `decl_shape` and `export_names` (declaration-level cutoff), `file_report`.
The scheduler of `check_project` keeps the order and asks the layer for each first-pass unit.
Measured on 1,000 files (release, opt-level 1, 2 cores): nothing changed 7-8 ms against 62-67 ms
for the full driver; a body edit 10.6 ms with 1 unit run; a signature edit 11.5 ms with 4 units run;
the cold check is 143-156 ms, slower than the full driver. 1387 tests pass under `-D warnings`.
Ownership decision for the parse: see the note; nothing keeps a tree between revisions.

Before implementing the full query layer, verify the behavior of the pinned Salsa version:
early cutoff, query reuse, thread handles, and cancellation.

Then introduce file inputs, module-header queries, resolution and graph queries, checking-unit
queries, and declaration projections. Use the declaration reads already recorded in
`FileReport.reads` to connect semantic dependencies to incremental recomputation.

Each row of the LLD's invalidation matrix should become a test that records which queries
execute and compares the incremental result against a cold check.

The key distinction is between reusing allocated capacity and reusing semantic results. The
current session and worker reuse behavior does not itself establish incremental checking.

**Carried over from E4 (phase 3b).** The E7 design has to say who owns a parsed tree and its
arena: a file input query, a header query, or the checking unit. Settle that first, then decide
whether discovery's parse can be reused by the checker. The E4 measurements (section 3, E4)
give the baseline to compare against: parsing once saves about 7.5 % of run time on the corpus
and would roughly double peak memory if every tree were kept.

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

- Statement recursion: `check_statement` now has the stack guard. Not yet re-run on a release
  build at 100,000 levels *(not verified)*.
- The three parser shapes (ternary chain, arrow chain, nested `Array<...>`) are now counted by
  `src/source_depth.rs` and refused before the parse, like bracket nesting. `tests/depth_guard.rs`
  covers them through the real binary in a debug build; the 100,000-level run on a release binary
  is not done. The count is still not a parse, so a regular expression holding a quote or backtick
  can skew it for a line.
- Instantiation depth is capped (`MAX_INSTANTIATION_DEPTH`); past it the result is cut and the file
  gets the `TSR9102` warning once. A real TS2589 error needs a source position and is not done.
- Budgets for recursive work, instantiation, and memory are still undefined.
- Run Miri or ASan on appropriate arena and reset paths, add a compile-fail ownership check if
  the arena design supports it, audit nondeterministic hash-map iteration in output paths (the three flagged uses are now
  annotated as order-independent), and implement the remaining LLD metrics (not started). Miri and
  ASan are not installed here; `src` has no `unsafe`.

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
   publication, cancellation, and read-once. Single-parse execution is deferred to E7.
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
5. **E4 / E7:** whether to parse each file once (phase 3b). Open on purpose: decide with the E7
   ownership design, using the measurements in section 3 under E4. Not needed for E4's exit
   condition.
6. **E4:** whether to parallelise discovery. Needs a multi-core measurement first.
