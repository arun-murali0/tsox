# The Project Scheduler: Dependency Counts, One Read per File, and a Parse Not Shared

> **Architecture case study:** replacing a scheduler that waited at every layer boundary with one that starts a unit the moment its own dependencies are done, finding that every file was being checked twice, and deciding with measurements not to share a parse between discovery and the checker.

[module-resolution](module-resolution.md) builds a graph and orders it into layers. [cycles-and-settlement](cycles-and-settlement.md) says what a unit is. This document is about running the units: in what order, on how many threads, reading what, and for what cost.

## 1. How it evolved

| When | Change | Commit | Why |
| --- | --- | --- | --- |
| 2026-10-20 | The dependency-count scheduler | `9910e2d` | A layer barrier makes a unit wait for slow units it does not depend on |
| 2026-10-20 | The project check runs on the ready queue, and no file is checked twice | `c44ce7c` | The queue existed, but the project check did not use it, and a second pass ran on every project |
| 2026-10-20 | Each file is read once: discovery keeps the text it read | `a354fc3` | The checker read again what discovery had already read and hashed |
| 2026-10-31 | The query layer asks the same units one at a time | `c12bc9a` | See [query-layer](query-layer.md) |

## 2. From layer barriers to a ready queue

**Problem.** The first scheduler ran the graph's layers in parallel and waited at the end of each. A layer finishes when its slowest unit does, so a unit with every dependency done still sat idle behind an unrelated slow one.

**Picked.** A queue that operates on components, not on files, so it keeps the unit contract from [cycles-and-settlement](cycles-and-settlement.md). Each unit has a count of the units it waits for. When the count reaches zero the unit is ready, and a worker takes it. Nothing waits for a layer.

`crates/tsox-project/src/module_resolution/schedule.rs` holds `run_units`. Its contract:

- A unit reads only the results of the units it waits for, and nothing else that another unit is producing.
- Outcomes come back indexed by unit, so the report does not depend on the order units finished in.
- A panic in a unit is recorded as that unit's result, and its dependents still run. It becomes an internal-error report marked incomplete for its files (`CheckerCrashed`), never a result that looks complete.
- The cancel token is checked as each unit starts. `check_project_cancellable` passes one through the driver, and `ProjectReport::cancelled` says a run was stopped.
- Units that wait on each other are refused before anything runs.

**The exit test.** On randomly generated graphs, every unit executes exactly once, after its dependencies complete; independent units can overlap; the output is the same whatever the timing. `tests/scheduler.rs` runs it at one, two and eight threads, and `random_projects_give_the_same_report_whatever_the_timing` runs it through the real checker. The corpus abort test stays green. The graph generator that the component search was tested with is reused for the random graphs.

## 3. Running the whole project on the queue

**Picked.** `check_project` runs the components of the graph on that queue. A file reads only what the components it imports published, plus a `Globals` value fixed before the pass: the ambient modules, augmentations, UMD namespaces and their declarations. Ambient providers come from the parse that discovery already made, so a declaration file is not parsed again for them.

**Evidence.** The 10,083 corpus files checked as one project give byte-identical output before and after the change, and 40 smaller projects agree with `--partial --cycles`.

### 3.1 The finding: every file was checked twice

A second pass exists for projects that use UMD globals or augmentations, since those change what other files can see. Its test asked whether the UMD table was empty, and the table got an entry for every file, so the answer was always "not empty". Every project was checked twice.

The second pass now runs only when the first found a UMD alias, an augmentation, or an ambient module outside a provider file. On the corpus as one project, in a release build, the check took about 2.3 s before and about 1.0 s after (two runs each). A later change narrows the second pass further, to the files that can change; see [library-and-ambient-declarations](library-and-ambient-declarations.md).

### 3.2 A behaviour change that came with it

The double pass had been giving an unsettled cycle twice its round budget by accident. With the bound forced very low, a file of such a cycle can now keep an unresolved-annotation warning. It still never reports a missing name or module, and `an_unsettled_cycle_reports_no_missing_name_among_its_own_members` checks exactly that. With the default bounds nothing changes: a cycle gets one round per member plus a spare.

## 4. Read each file once

**Problem.** A file was read at least twice: once in discovery to find its edges, and once in the checker. Discovery already fingerprinted and hashed every file (`FileState`, `content_hash`).

**Picked.** Discovery keeps the text it read, for a scanned file whose bytes are valid UTF-8, and `check_file` uses that text instead of reading the file again (`ModuleGraph::source`, `load_text`). A file with no kept text is read by the checker as before, so a file that is not valid UTF-8 still fails to read. `tests/source_once.rs` has five tests.

**Two consequences.**
- The checker checks the text discovery saw, not whatever is on disk later. A file edited after the walk is still reported as it was seen, and `changed_files` is what says it has moved.
- Every scanned file's text stays in memory for the life of the graph. Text only, no syntax trees.

## 5. Parse once: measured, and not done

A file is still parsed twice: once by discovery to find its imports, and once by the checker, which then runs semantic analysis. A declaration file could be parsed a third time for the ambient providers, until section 3 removed that.

**Measured** on the 10,083-file corpus as one project, release build, one thread (2026-10-20):

| Step | Time |
| --- | --- |
| Discovery scan (parse and import walk) | 0.111 s |
| The checker's parse alone | 0.089 s |
| The checker's semantic analysis | 0.072 s |
| `ModuleGraph::build` | 0.20 to 0.22 s |
| `check_project` | 0.96 to 0.98 s |
| The whole pipeline | about 1.18 s |

The checker's repeat parse is about 0.089 s of 1.18 s, roughly 7.5 %, which is the most that parsing once could save. Keeping every discovery tree alive until the checker uses it holds 73 MB of syntax-tree data (227 MB of allocator capacity, since each file has an allocator of its own) and raises peak resident memory from about 70 MB to about 139 MB. The trees are about 11 times the size of the source text, so the cost grows with the project. About 1,100 of the files have parse errors, so the analysis time covers the rest.

**Decision.** Do not parse once. The gain is small against the memory, and against an ownership design that would have to say who holds a parsed tree and for how long. The query layer settled that ownership the other way: the header query scans a file for its requests and keeps no tree, and the checking unit parses its own file and drops the tree with its arena. Nothing keeps a tree between revisions, so peak memory is not doubled.

**Not measured.** The machine had one core. The checker runs on all cores and discovery runs on one thread, so on a multi-core machine discovery could be a much larger share of the wall-clock time than it is here. Parsing once would not change that, because it removes the checker's parse and not discovery's. Parallelising discovery is the more likely gain, and it needs a multi-core run to confirm.

## 6. What is still not done

- Parallel discovery. It needs a multi-core measurement first.
- A measurement of the whole pipeline on more than one core.

## 7. Invariants to keep

1. **A unit starts when its own dependencies are done.** Never at a layer boundary.
2. **A unit reads only what it waits for.** Nothing another unit is still producing.
3. **The report does not depend on finishing order.** Outcomes are indexed by unit and published in a fixed order.
4. **A panic is a result.** It becomes an incomplete internal-error report, and the dependents of that unit still run.
5. **A cancelled run says so.** `ProjectReport::cancelled` is set, and nothing partial is passed off as complete.
6. **Every unit runs exactly once.** The second pass is the exception, and it runs only for the files that can change.
