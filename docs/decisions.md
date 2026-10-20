# Decisions

> **Architecture case study:** what was decided at stage 0, why, and what would reopen it.

The design is in `HLD.md` and `LLD.md`. This page is the short list of what was left open there and is now closed, so a later change can check itself against it. Each decision stays as it is until the condition in the last column happens.

| # | Decision | Reason | Reopen when |
| --- | --- | --- | --- |
| 1 | `DeclKey` is identity, `SigHash` is version | A signature inside the id changes the id of every referrer on each edit, which defeats early cutoff | Never expected; this is the base of ADR-5 |
| 2 | Aliases are transparent wrapper nodes (`Named`) | A side table keyed by id is what forces `set()` and `make_unique` today | Stage 1 shows the wrapper costs more than the table it replaces |
| 3 | Union display order is first writer wins, in a side table | Identity stays canonical and messages keep the written order, closest to tsc | The tsc comparison shows order mismatches that matter |
| 4 | The planned unit of work is one SCC. Built so far: a unit is one file, and a cycle's members are checked again, each round reading the previous snapshot, at most four rounds | Planned: cycle members share one arena, so nothing crosses arenas inside a cycle. Built: the members share a layer and the first pass found nothing among them, so they are iterated (`cacdbdb`). The commit does not record why iteration was preferred to one arena per component | The choice between the two is made on purpose, or S4 shows one SCC dominating wall time |
| 5 | Hash function is XXH3-128 | Already a dependency, and fast | Input must be treated as adversarial |
| 6 | The hash is computed lazily, at an export boundary | Most types never leave their file | S2 shows lazy and eager disagree |
| 7 | Salsa is pinned at `=0.28.5`, provisionally | It is what the foundation layer builds on | S1 fails |
| 8 | A module key follows the rule in LLD 3.1.1 | An id has to survive moving the project and a different install layout. Implemented in `src/keys.rs` (`45b6098`) | A platform test shows two spellings of one file get two keys |
| 9 | `FileId` is a run-local index, sorted by canonical path | Output order must not depend on discovery order, and ids must not be stored | Never expected; it only works if no key uses it |
| 10 | Intersection identity is ordered, deduplicated by id, never sorted | Call-signature order depends on member order, and callability needs a body that may not exist yet. tsc does the same (probed on 5.9.3; the comparison tooling is now on 6.0.3) | A fixture shows overload order through intersections is not needed |
| 11 | Intersection reduction has two levels: ids and wrappers at construction, bodies lazily | A decision that read a body while a declaration was unresolved would bake a wrong answer into the intern table | Never expected; it is the placeholder bug turned around |
| 12 | A product of union sizes of 100,000 or more is TS2590 | Same cap as tsc. Reported as `TSR1701`, mapped to TS2590 (`d3662d7`), where it used to become the error type silently | The ADR-6 budgets need it lower |
| 13 | A new `Type` variant goes at the end of the enum | Adding `Intersection` after `Union` changed an unrelated diagnostic count from 5 to 1. The cause was a digest collision between two literals, not the order; it was fixed in `91454fa` and nothing depends on the order any more. The variant still goes last, which costs nothing | Never expected to matter again |
| 14 | A bare `return f(...)` inside `f` is not a return-type read | tsc leaves a call to the function itself out of the return type, so `return gcd(b, a % b)` has no error under tsc 6.0.3 and gets none here. A self call inside a larger expression (`n * f(n - 1)`), as an argument of another call, or through a second function is still a read and still gets TS7023. The inferred type of the bare call stays the placeholder `any`, where tsc gives the type of the other returns | Inferring the return type ignores the bare call the way tsc does, which needs an answer for a function that returns only itself (tsc gives `never`) |
| 15 | Units start on a dependency count, and results are read by unit index | A layer barrier makes a unit wait for the slowest unit of its layer. The scheduler starts a unit when the last unit it waits for finishes, a unit reads only those results, and the outcomes come back indexed by unit, so nothing depends on finishing order. A panic or a cancel is recorded per unit and dependents still run | The export tables are merged in an order that depends on finishing order, which means publication has to be ordered by unit index first |

## The module key, in one place

```text
Project   : path relative to the project root, "/" separators
Package   : package name, version, path inside the package
External  : absolute canonical path
```

A file under `node_modules` is a Package file when its nearest `package.json` has both a name and a version, and an External file when none does. A workspace package or a linked package resolves outside `node_modules`, so it is a Project file. Nothing in a key comes from a `FileId`, the absolute root or a timestamp.

The cost is accepted and written down: two copies of one name and version with different contents, one patched in place, share a key.

## What this changes in the code

Decision 9 is the first one that does. `ModuleGraph::build` still walks the project with discovery ids, but only until the walk ends. It then sorts the discovered paths and renumbers every file and every edge target, so the ids it publishes follow path order and no longer depend on the order the entries were given in. `ProjectFiles` itself is unchanged and still hands out ids in the order it is asked.

Decisions 5 and 8 are implemented in `src/keys.rs` and `src/stable_hash.rs`. How the keys are used, and where the code differs from decision 4, is in [cross-file-types](architecture/cross-file-types.md).
