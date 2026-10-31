# Architecture Foundation

> **Architecture case study:** bringing in the three dependencies the design documents name, as small pieces that can each be switched off, before any stage depends on them.

## 1. Why this is a separate milestone

`HLD.md` settles three things this milestone makes real: bump memory for transient data with strict rules (ADR-2), Salsa to decide what must be recomputed (ADR-4). The stages that use them come later, in the HLD's order: the project driver at stage 3, the Salsa layer at stage 4, the scheduler at stage 5.

Nothing in the checker uses any of them yet. What exists is a small, tested piece for each, so the stage that needs one starts from something already pinned down instead of from a dependency that was never exercised. There are 13 unit tests next to the code and 3 integration tests in `tests/module_resolution.rs`.

It does not move a single type, query or diagnostic onto Salsa, and it does not touch the arena. Section 8 lists what is left.

## 2. One feature per dependency

| Feature | Pulls in | Adds |
| --- | --- | --- |
| `incremental` | `salsa` | `IncrementalDatabase`, `SourceFile`, `source_len` |
| `scratchpad` | `bumpalo` | `WorkerScratch` |
| `mimalloc` | `mimalloc` (not on wasm32) | the global allocator of the `tsox` binary |

Every one is off by default. A default build, the WASM build and the binary that CI builds and compares against tsc have the same dependencies as before.

## 3. Petgraph: removed (E2)

`ModuleTopology` and `ModuleGraph::topology()` were a petgraph snapshot of the module graph. Nothing in `check_project` or the CLI ever read it, and the scheduler orders by `ModuleGraph::sccs` and `unit_dependencies`, so E2 removed the view and the `petgraph` dependency rather than keep a second representation with no production job. The graph's own iterative Tarjan (`src/scc.rs`) is the single implementation; `the_components_come_dependencies_first` in `tests/module_resolution.rs` asserts the component order LLD 4.1 asks for.

## 4. Salsa: the shape from the LLD, and nothing more

`SourceFile` follows LLD 6.2: a path, the text as an `Arc<str>` so a worker can hold it without copying the file, and a content hash. The file id is stored as a plain `u32`, which keeps `FileId` out of Salsa's handle types until a query needs it.

`Database::upsert_source` carries the rule from LLD 6.1:

```text
hash equal to the stored one -> change nothing, no new revision
otherwise                    -> rewrite text and hash on the existing input
```

The input is updated in place. A second input for the same `FileId` would leave the old one behind and every query over it would be recomputed for nothing. `a_save_without_edits_does_not_rerun_the_query` counts query executions through Salsa's event hook, so it checks that a result is reused and not only that it comes back correct.

Durability follows LLD 6.4: files under `node_modules` are high durability and project files are low, so a query that reads only dependency files should not be re-verified while project files are edited. That is a property of how Salsa verifies queries, not something an execution count can show, so no test claims it; the setting is there so the later queries inherit it.

Salsa-interned names are not included. ADR-1 chose per-file bump arenas over Salsa-interned types, and a Salsa-interned name type would be a second identity for the same thing.

## 5. The scratchpad: only values that cannot leak

`WorkerScratch` wraps a `Bump` for data that is built, used and discarded inside one job. ADR-2 makes the rule that nothing in a bump needs `Drop`, because a `Bump` never runs destructors and a value that owns heap memory leaks it on every reset. `alloc` therefore accepts only `Copy` values, which cannot have a destructor, and text goes in through `alloc_str`.

- **Not for the AST.** Oxc parses into its own allocator, and an Oxc AST cannot live in another one, so the two are separate.
- **`allocated_bytes` is public.** dhat sees only allocations that go through the global allocator, not a bump's chunks (LLD 2.3), so a memory report has to add them itself.
- **`reset` keeps the memory.** `a_reset_keeps_the_memory_for_the_next_job` checks that the second job of the same size does not grow it.
- **One lint allowed, with a reason.** `alloc` returns `&mut T` from `&self`, which `clippy::mut_from_ref` rejects. Each allocation is fresh and disjoint memory, so nothing aliases.

## 6. The allocator belongs to the binary

mimalloc is the global allocator on native targets (ADR-2). It is set in `bin/tsox.rs` behind the `mimalloc` feature and not in the library, because a library that chose a global allocator would change it for every program that links the crate.

The feature is off by default so the binary CI builds does not change. A release build opts in with `--features mimalloc`, and moving that into the CI build command is a one-line change when the timings are ready to be compared. LLD 2.3 applies: compare benchmark results only between runs that use the same allocator. The dependency is declared for non-wasm targets only, so the WASM build does not resolve it.

## 7. Benchmarks

`benches/foundation_benchmark.rs` reuses the projects from `benches/support/project_fixtures.rs`. It times petgraph's cycle search next to the graph's own on the same ring of 50 files, building and componentising the topology of the layered project, a job's worth of scratch values against a `Vec` that is dropped, and the incremental layer on 200 files: an unchanged save and a read of a result that is still valid. It needs all three features.

## 8. What is still not done

- **The Salsa queries.** Module header, resolved imports and per-unit checking (LLD 6.2) are not written; `source_len` only gives the layer something to memoize and test against.
- **`Project` and `resolution_epoch`.** The input that invalidates resolver results when a file is added, removed or renamed (LLD 6.4) does not exist.
- **The invalidation matrix.** Every row of LLD 6.5 has a test (`tests/incremental_project.rs`); see `query-layer.md`.
- **Edge kinds.** The topology's edges carry no weight yet (LLD 4.2).
- **Any consumer.** Nothing in `check_project` or the CLI reads the topology, the scratchpad or the database.
- **mimalloc in CI.** The CI build and the tsc comparison still run on the platform allocator.
- **Clap subcommands.** The CLI is the existing one; nothing here changes its arguments or output.

## 9. What Architecture Foundation completed

- petgraph as an optional dependency, behind a view that agrees with the graph's own cycle search
- a Salsa input and first query shaped like LLD 6.2, with the unchanged-save rule and durability by file owner
- a bump scratchpad that accepts only values that cannot leak
- mimalloc as the binary's allocator behind its own feature
- a benchmark for each piece, and no new dependency in a default build

The architecture lesson is: **a dependency should arrive as a small, tested piece behind its own switch, shaped the way the design says, before any stage needs it.**
