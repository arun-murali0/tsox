# The Project Engine: Checking a Set of Files

> **Architecture case study:** how a checker that was built one file at a time was made to check a project, in what order the pieces were stabilized, and what had to be true before the type system could change without the project driver changing with it.

A file is checked in an arena of its own (see [arena](arena.md)). A project is many files that import each other, that declare globals and augment each other's modules, that are configured by a `tsconfig.json`, and that change while an editor is open. The **project engine** is everything between "a set of files on disk" and "a report", and this page is the map of it.

## 1. What the engine is for

The engine is responsible for module resolution, dependency tracking, scheduling units of work, publishing a file's declarations to the files that use them, publishing results in a fixed order, and re-checking only what a change reaches. The goal was to give these parts predictable behaviour *before* the type system changed (type representation, generic inference, the arena), so that later work on types would not keep changing the project driver.

The goal was not to freeze the implementation. It was to establish contracts: what a unit is, what it may read, what it publishes, and what a failure looks like. Behind those contracts the type system can change.

**What it does not do.** It does not rewrite the type arena, the generic solver or the type representation. The existing implementation stays the baseline until a specific change is justified by a correctness requirement or by a measurement.

## 2. The parts

| Part | The question it answers | Document |
| --- | --- | --- |
| Safety net and limits | Can any input abort the process, and does the output depend on timing? | [safety-net-and-limits](safety-net-and-limits.md) |
| Partial results | What does a checker do when the list of a file's exports is known only in part? | [partial-results](partial-results.md) |
| Cycles and settlement | Which edges order the work, and how is a group of files that import each other checked? | [cycles-and-settlement](cycles-and-settlement.md) |
| The scheduler | In what order, on how many threads, reading what? | [scheduler](scheduler.md) |
| Project configuration | How does `tsconfig.json` select files and steer resolution, and when may a package be called missing? | [project-configuration](project-configuration.md) |
| Library and ambient declarations | How do names that are not imported reach the checker? | [library-and-ambient-declarations](library-and-ambient-declarations.md) |
| The query layer | How is only what a change reaches checked again? | [query-layer](query-layer.md) |

Beneath these sit [module-resolution](module-resolution.md), which builds the graph and its layers, and [cross-file-types](cross-file-types.md), which says how a declaration moves between arenas.

## 3. The order it was done in, and why

1. **Measure partial results first.** Evidence before redesign: it says what the export model lacks, in order, without changing anything.
2. **Fix the unit contract before the scheduler.** Whether a cycle is one unit decides what a scheduler schedules. Keep iteration, state the contract, test it.
3. **Then the scheduler and the pipeline.** Dependency counts, deterministic publication, cancellation, one read per file.
4. **Configuration beside it.** It is largely independent of the scheduler and removes errors caused by incomplete project inputs.
5. **Library and ambient declarations.** Names that are not imported, in a way that does not depend on file order.
6. **The query layer, last of the structure.** Reuse needs stable units and stable publication, or a cache serves stale answers.
7. **Safety throughout.** Limits and the corpus guarantee are not a final phase; a recursion guard added late would be added to every walk at once.

The order is deliberate. Measurement supplies the evidence, the cycle contract fixes the unit, the scheduler stabilizes execution, configuration and libraries supply realistic inputs, and the query layer adds reuse on top of those contracts.

## 4. The exit gate

The engine's work is done when each line is true. The status is as last recorded; the last column says how to check it again.

| Condition | Status | How to check |
| --- | --- | --- |
| The whole agreed corpus completes with no process abort | Met (last full-stride run 2026-10-20) | `TSOX_CORPUS_STRIDE=1 cargo test --test corpus_aborts` |
| Output is byte-identical across supported thread counts | Met | `crates/tsox-cli/tests/determinism.rs` |
| Dependencies complete before consumers, and each unit runs exactly once | Met | `tests/scheduler.rs` and the random-project test through the real checker |
| Cyclic modules settle, or give a controlled incomplete result | Met | `tests/scc_contract.rs`, `tests/circular_returns.rs` |
| A body-only edit re-checks only the unit it is in | Met in incremental mode | `tests/incremental_project.rs`; see [query-layer](query-layer.md) |
| Incremental results agree with a cold check | Met | `tests/incremental_project.rs` compares random edits with a cold run |
| Baseline timing, allocation and memory are recorded | Partly: benchmarks and the harness measure from outside; the checker records no metrics of its own | `benches/`, `scripts/harness.sh` |
| Supported configuration and declaration-loading cases give no false missing-module or missing-name error | Partly: configuration and ambient declarations are done; real generic library shapes are not | [library-and-ambient-declarations](library-and-ambient-declarations.md) |

Performance changes are judged against a recorded baseline. Correctness and deterministic output stay mandatory even when an optimization is faster.

## 5. What it taught

- **Measure before extending.** Counting root causes of partial results, not affected files, showed that one reason accounts for most of them.
- **A test that is always true hides a cost.** A second pass meant for a few projects ran on every one, because its guard asked whether a table was empty and the table was never empty. Fixing it cut the check time on the corpus from about 2.3 s to about 1.0 s.
- **Quiet beats wrong.** A partial list, a loose `any`, an unresolved name and an unsettled cycle all cost completeness and never correctness. Every part of the engine falls back the same way.
- **Reverse the order in the test.** Every table that depends on files is built in path order, and a test lists the files the other way round.
- **Decide with numbers.** Parsing each file once would save about 7.5 % and nearly double peak memory; it was not done.
- **A limit that says so.** A file refused for nesting, a cut-off instantiation and an incomplete export are reported as such, not guessed.

## 6. What is open

- Real generic shapes for the standard library, through the provider. It needs generic inference.
- Whether the quadratic and cubic cases listed in [safety-net-and-limits](safety-net-and-limits.md) are worth fixing before budgets for recursive work exist.
- Parallel discovery, which needs a measurement on more than one core.
- Whether `scripts/isolate-fixture-aborts.sh` stays. The corpus test covers the same ground inside `cargo test`; the script is faster (about 135 s against 216 s) and runs on a release binary.
