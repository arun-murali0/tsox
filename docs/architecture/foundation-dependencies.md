# Architecture Foundation: Salsa, Bumpalo and mimalloc

> **Architecture case study:** bringing in the dependencies the design needed as small pieces that can each be switched off, before any stage depended on them, and what became of each one.

## 1. Why this was a separate milestone

Three decisions were taken before the code that needed them:

- **Per-file arenas, not Salsa-interned types.** A type is an id into an arena owned by one file, and a type that crosses files travels as data (see [arena](arena.md) and [cross-file-types](cross-file-types.md)). A Salsa-interned name type would have been a second identity for the same thing.
- **Salsa decides what must be recomputed.** Not what a type is, only which unit of work has to run again after an edit.
- **Bump memory only for transient data, with a strict rule,** and a faster global allocator in the binary.

The dependencies behind those decisions (`salsa`, `bumpalo`, `mimalloc`, and at first `petgraph`) arrived on 2026-10-04 (`22eae2e`) as a small, tested piece for each, so the stage that needed one started from something already pinned down and not from a dependency that had never been exercised. This page says what each piece is, what became of it, and what it still does not do.

## 2. One feature per dependency

| Feature | On | Pulls in | What it adds | Used by |
| --- | --- | --- | --- | --- |
| `incremental` | `tsox` (turns on `module-resolution`) | `salsa`, in `tsox-db` | `IncrementalDatabase`, `SourceFile`, and the query layer | The CLI's `--incremental` and `--watch`; see [query-layer](query-layer.md) |
| `scratchpad` | `tsox-base`, forwarded by `tsox` | `bumpalo` | `WorkerScratch` | Benchmarks and the allocation profile; nothing in the checker |
| `mimalloc` | `tsox-cli` | `mimalloc` (not on wasm32) | The global allocator of the `tsox` binary | The release build, when asked |

Every one is off by default. A default build, the wasm build and the binary that CI builds and compares against `tsc` have the same dependencies as before.

## 3. Petgraph: removed

`ModuleTopology` and `ModuleGraph::topology()` were a `petgraph` snapshot of the module graph. Nothing in `check_project` or the CLI ever read it: the scheduler orders by `ModuleGraph::sccs`. It was removed, together with the `petgraph` dependency, rather than kept as a second representation of the graph with no production job. The graph's own iterative Tarjan search (`crates/tsox-project/src/scc.rs`) is the single implementation.

The lesson is the one in section 8: a dependency that has no consumer is a cost with no gain, and the way to find out is to try to use it.

## 4. Salsa: from one input to a query layer

**The first piece.** `SourceFile` is a Salsa input: a path, the text as an `Arc<str>` so a worker can hold it without copying the file, a content hash, and the file id stored as a plain `u32`, which keeps `FileId` out of Salsa's handle types until a query needs it. `Database::upsert_source` carries one rule:

```text
hash equal to the stored one -> change nothing, no new revision
otherwise                    -> rewrite text and hash on the existing input
```

The input is updated in place. A second input for the same `FileId` would leave the old one behind, and every query over it would be recomputed for nothing. `a_save_without_edits_does_not_rerun_the_query` counts query executions through Salsa's event hook, so it checks that a result is reused and not only that it comes back correct.

**Durability follows the file's owner.** Files under `node_modules` are high durability and project files are low, so a query that reads only dependency files is not re-verified while project files are edited. That is a property of how Salsa verifies queries and not something an execution count can show, so no test claims it; the setting is there so later queries inherit it.

**What it became.** The first piece had one input and one demonstration query (`source_len`). The module header, per-component check, declaration projections and per-file report queries now exist in `tsox-db`, with an invalidation test for each case. [query-layer](query-layer.md) tells that story, and the eight tests in `crates/tsox-db/src/incremental.rs` still pin the Salsa behaviours the design relies on.

## 5. The scratchpad: only values that cannot leak

`WorkerScratch` wraps a `Bump` for data that is built, used and discarded inside one job. A `Bump` never runs destructors, so a value that owns heap memory leaks it on every reset. The rule is therefore that nothing in a bump needs `Drop`: `alloc` accepts only `Copy` values, which cannot have a destructor, and text goes in through `alloc_str`.

- **Not for the syntax tree.** Oxc parses into its own allocator, and a tree cannot live in another one, so the two are separate.
- **`allocated_bytes` is public.** `dhat` sees only allocations that go through the global allocator, not a bump's chunks, so a memory report has to add them itself.
- **`reset` keeps the memory.** `a_reset_keeps_the_memory_for_the_next_job` checks that the second job of the same size does not grow it.
- **One lint allowed, with a reason.** `alloc` returns `&mut T` from `&self`, which `clippy::mut_from_ref` rejects. Each allocation is fresh and disjoint memory, so nothing aliases.

## 6. The allocator belongs to the binary

mimalloc is the global allocator on native targets. It is set in `crates/tsox-cli/src/main.rs` behind the `mimalloc` feature of the `tsox-cli` crate, and not in the library, because a library that chose a global allocator would change it for every program that links the crate.

The feature is off by default so the binary CI builds does not change. A release build opts in with `cargo build --release -p tsox-cli --features mimalloc`; moving that into the CI build command is a one-line change when the timings are ready to be compared. Compare benchmark results only between runs that use the same allocator. The dependency is declared for non-wasm targets only, so the wasm build never resolves it.

## 7. Benchmarks

`benches/foundation_benchmark.rs` reuses the projects from `benches/support/project_fixtures.rs`. It times a job's worth of scratch values against a `Vec` that is dropped (same work and same count, so the difference is the allocator and the free), and the incremental layer on 200 files: an unchanged save, and a read of a result that is still valid. It needs the `module-resolution`, `incremental` and `scratchpad` features.

## 8. What is still not done

- **A consumer for the scratchpad.** Nothing in the checker allocates from `WorkerScratch`; only the benchmarks and the allocation profile do.
- **mimalloc in CI.** The CI build and the comparison with `tsc` still run on the platform allocator.
- **A static library arena.** Each file's arena is built per unit and the standard library is not one of them; see [library-and-ambient-declarations](library-and-ambient-declarations.md).

## 9. What Architecture Foundation completed

- `salsa` as an optional dependency with an input, an update rule and durability by file owner, which grew into the query layer
- a bump scratchpad that accepts only values that cannot leak
- mimalloc as the binary's allocator behind its own feature
- a benchmark for each piece, and no new dependency in a default build
- `petgraph`, tried and removed when nothing used it

The lesson: **a dependency should arrive as a small, tested piece behind its own switch before any stage needs it, and should leave again if no stage does.**
