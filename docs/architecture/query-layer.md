# The query layer (E7)

The design this implements is in [`e7-system-design-plan.txt`](e7-system-design-plan.txt) (kept as
written) and LLD 6.2 to 6.7. This note says what was built, where it differs from the plan, and
what was measured. Code: `src/incremental.rs` (inputs, handles) and `src/incremental/project.rs`
(queries and the driver). Behind the `incremental` feature, which turns on `module-resolution`.

## Shape

Two planes, as planned. Salsa decides what must run again. The checker does the work and knows
nothing about Salsa: `check_unit_isolated` runs `run_unit`, the code the full driver runs for one
component, against a snapshot of what its dependencies exported. The scheduler of `check_project`
stays in charge of order (LLD 6.7, option 1); for each component of the first pass it asks the
query layer, and a component whose inputs did not change is answered from its memo.
`IncrementalProject::check()` returns the same `ProjectReport` as `check_project`.

| Plan | Built as |
|---|---|
| Declaration-level edges (`DeclKey`, `SigHash`) | `decl_shape(decl)` and `export_names(file)` projection queries. A unit reads its dependencies' exports from a store and then registers exactly the declarations (`FileReport.reads`) and the export lists of the modules it imports. An equal projection leaves every reader alone. |
| Micro invalidation (file edits) | `SourceFile` inputs with a hash; equal bytes start no revision. `module_header(file)` has no offsets, so an edit that only moves text is cut off before the graph. |
| Resolution epoch | `ProjectInput { bundle, epoch }`. The graph is rebuilt, and the epoch moves, when a file is added or removed, a header changes, or a config file changes. |
| Global scope epoch | Ambient providers (declaration files with `declare module`) are checked as their own queries (`check_provider`). Every unit query is keyed by a digest of what the providers declared, so a provider edit that declares something new reruns the units and one that does not (a comment) reruns none. Global declarations, UMD aliases and augmentations found by the first pass still go through the driver's second pass, which is not memoized: correct, not incremental. |
| LRU eviction | `check_scc` has `lru = 4096`; `set_unit_cache_capacity` changes it. An evicted unit is recomputed when asked; its dependents fall back to the unit's query. |
| Cooperative cancellation | Two paths. Salsa's own: a write cancels running queries and waits (pinned by the spike tests). And `check_cancellable(&CancelToken)`: the token reaches each unit; a stopped unit unwinds out of its query with Salsa's cancellation payload, so it stores nothing and runs again next time, while finished units keep their memos. A cancelled check returns `Err`, never a partial report. |
| Debounced input | The query layer takes buffers, not events: `set_buffer(path, text)` and `close_buffer(path)` give editor text that wins over the disk, for discovery and checking alike. Debouncing and the LSP wiring stay in the server. |
| Dual arena (static library arena) | Not built. The library is a name list; each unit's arena is per worker and cleared after the unit. |

## Two modes

The checker is one; two drivers run it.

| | Driver | Use |
|---|---|---|
| Default CLI and CI (`tsox --project`) | `check_project`, the full driver | One run: fastest cold, no memo overhead. |
| Editor, LSP, watch (`IncrementalProject`) | the query layer | A long-lived process: one slower first check, then each edit runs only what it reaches. |

The CLI keeps the full driver unless asked: `--incremental` runs the query layer once (same output and
exit code, tested; about twice the cost, so it is for trying it), and `--watch` checks, then checks
again whenever a file changes or is added or removed (it implies `--incremental`). A language server
uses `IncrementalProject` directly, with `set_buffer` for unsaved text. The full-driver-first idea
(seed the memo from a full run to remove the slower first open) is not built.

## Decisions

- **Who owns a parse.** The header query scans a file for its requests and keeps no tree. The
  checking unit parses its own file and drops the tree with its arena. Nothing keeps an AST between
  revisions, so peak memory is not doubled (the E4 measurement that argued against keeping trees).
  Parsing once for both stays open.
- **Order stays with the scheduler.** A unit reads its dependencies from a store, not through a
  query, because a query result would make it depend on all of the dependency. That is sound only
  when the dependency is current, which the scheduler guarantees (a unit waits for what it imports).
  `Bundle::verified` records it, and a unit that finds a dependency not verified in this
  generation calls the dependency's query instead.
- **A rebuilt graph keeps what it can.** Each unit has a plan (`scc_plan`): its members, their edges, the ids of everything it may read, the compiler environment. A unit whose plan is equal keeps its memo across the rebuild, so an import added to one file runs that file and what reads it. File ids follow path order and outputs carry them, so a file added early in path order moves the ids after it, and every unit that sees an id move runs again. Exactness is tested with random file and import edits against a cold check.
- **Handles are pooled.** Making a Salsa handle costs about 80 µs, more than answering a unit that
  did not change; the memo keeps one handle per worker thread.

## Evidence

`tests/incremental_project.rs` (16 tests) takes each row of the LLD 6.5 matrix, asserts which units
ran, and compares the report with a cold `check_project` on the same files; a run of 40 random
edits does the same after every edit. The Salsa behaviors it relies on are pinned in
`src/incremental.rs` (early cutoff, reuse, thread handles, cancellation).

| LLD 6.5 row | Test | Units that ran |
|---|---|---|
| Unchanged save | `an_unchanged_save_runs_nothing` | none, no header query, epoch unchanged |
| Whitespace or comment edit | `a_whitespace_edit_runs_that_unit_and_stops_there` | that unit |
| Body change, same signature | `a_body_change_with_the_same_signature_leaves_importers_alone` | that unit |
| Exported signature change | `a_signature_change_reruns_only_the_importers_that_read_it` | the file and the importer that reads the declaration |
| Another export changed | `an_importer_of_an_unchanged_declaration_stays_green` | the file and the importer of the changed one |
| Import added | `an_added_import_moves_the_epoch_and_rebuilds_the_graph` | graph rebuilt, epoch 1 |
| Deep dependency changes shape | `a_shape_change_runs_along_the_chain_until_the_output_is_equal` | along the chain, then stops |
| Change inside a cycle | `a_change_inside_a_cycle_reruns_the_whole_cycle_and_nothing_else` | the cycle's unit only |
| File added, removed | `a_file_added_or_removed_moves_the_epoch` | epoch 1, then 2 |
| Config change | `a_config_change_rebuilds_and_applies_the_option` | graph rebuilt, option applied |
| Global scope | `a_global_declaration_edit_gives_the_cold_answer` | cold answer (second pass, not memoized) |
| Incomplete unit | `a_file_that_does_not_parse_is_reported_and_fixed_by_the_next_edit` | see below |

**Incomplete units.** A cancelled or crashed unit never reaches a memo: the query layer passes no
cancel token, and a panic unwinds the query before it stores anything. A unit that is incomplete
but deterministic (a parse failure, a cycle that did not settle) is memoized like any other, and
rerunning it would give the same answer.

## Measured

1,000 generated files, each importing 3 earlier ones; release build (`opt-level 3`, fat LTO), 2 cores
(`cargo run --release --features incremental --example incremental_baseline -- <dir> <file> <kind>`).
Both columns were measured in the same runs.

| | Full driver | Query layer |
|---|---|---|
| Cold check, all units | 31-35 ms | 61-66 ms |
| Check, nothing changed | 31-35 ms | 5 ms |
| After a body, signature or comment edit | 31-35 ms | 6-7 ms (1 unit ran for a body edit) |

The cold check is about 1.9 times the full driver, down from 2.3: each unit still rebuilds a table of
its transitive dependencies' declarations (the names are shared, not copied). That is the cost of
isolating units; a project this size is cheap cold already, so the gain is on edits. To be measured
again on 10,000 files and on the corpus.

`scripts/harness.sh --tsox-only` runs the real-project harness without tsc and tsgo: speed, memory,
errors by code, partly checked files, and whether two runs agree. On Zustand (31 files checked, 220 in
the graph): 276 ms median wall, 26 MB peak, deterministic output.

## Limits

- A file added or removed early in path order moves later file ids, so the units that see one move run again.
- A buffer is found by an import only if the file exists on disk; resolution looks at the disk.
- The second pass for global declarations is not memoized, and `CheckStats` does not count it.
- Cancellation is checked between statements and at unit boundaries, not inside one expression.
- Not built: the static library arena, and keeping a parse between revisions.
- Cold cost: see Measured.

## Soundness invariants and known limits

`check_scc` reads the shared `Bundle` (the per-unit `verified` generation flags and the stored outputs)
outside Salsa's tracking. It is correct only while these hold:

1. A unit runs only after every unit it waits for has finished and stored its output (the external
   scheduler's order).
2. `verified[dependency] == generation` means the stored output belongs to this generation; if it does
   not, the unit calls the dependency's query instead of reading the store.
3. Stored outputs are held weakly. An output evicted by the LRU makes the unit call the dependency's
   query; it never reads a stale one.
4. The unit plan is read through a tracked query, so a changed plan changes the memo key.

If the scheduler or the generation protocol changes, the guard is the equality-with-cold-run tests in
`tests/incremental_project.rs`, the random-edit one above all. Run them first.

Known limits, stated so they are not mistaken for gains:
- An edit in a file that declares globals (a script, `declare global`, a UMD alias, an augmentation)
  gives the cold answer through the second pass, which is not memoized; a project with many such files
  gains little.
- The measurements above are for 1,000 generated files. Nothing here is measured at 10,000 files or on
  the corpus yet.
- `incremental` is an opt-in Cargo feature; the default CLI mode is the full driver.

## Soundness contract for the untracked reads

`check_scc` reads the epoch's `Bundle` (the per-unit `verified` generations and the stored outputs)
outside Salsa's dependency tracking. That is sound only while these hold, and a change that breaks
one must be caught by `tests/incremental_project.rs`, which compares every row of the invalidation
matrix, and a 40-edit random run, with a cold `check_project`:

1. A unit reads a dependency's stored output only if that dependency was verified in this
   generation; otherwise it calls the dependency's query, which recomputes or reuses it properly.
2. A stored output is a `Weak`: an evicted one (LRU) falls back to the query, never to a stale value.
3. The scheduler still orders units by dependencies, so a dependency is verified before its readers.
4. A cancelled or panicking unit unwinds before it stores anything.

Known limit: an edit that changes a global declaration (a script's names, a UMD alias, an ambient
module, an augmentation) takes the cold second-pass path and is not memoized, so a project with many
globals gets little of the incremental gain.

