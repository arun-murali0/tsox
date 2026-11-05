# Cycles and Settlement: Edge Kinds, and One Component as One Unit

> **Architecture case study:** what counts as a dependency between files, how a group of files that import each other is checked, and why it is settled by repeating the check until the exports stop changing and not by giving the group one shared arena.

[module-resolution](module-resolution.md) builds the graph and finds its cycles. [scheduler](scheduler.md) runs the units. This document is about the two questions between them: which edges order the work, and what a strongly connected component of files is, as a unit of checking.

## 1. How it evolved

| When | Change | Commit | Why |
| --- | --- | --- | --- |
| 2026-10-15 | The recheck bound grows with the cycle; cycles that do not settle are reported | `ab60061` | A fixed small bound was too small for a long ring of re-exports |
| 2026-10-19 | Files that load a file with a dynamic `import()` are recorded | `ea48438` | What has to rerun when a file changes is more than the files that order after it |
| 2026-10-19 | Each cycle is settled on its own, unsettled files are named, circular inferred returns are reported | `1fe24cf` | One cycle that did not settle marked an unrelated one incomplete, and a circle of inferred returns passed as `any` |

## 2. An edge is a dependency that affects checking

**Problem.** A graph with an edge for every import-shaped expression orders work that does not need ordering and creates cycles that are not cycles.

**Picked.** The graph keeps every edge, and every kind but one is a dependency. Static and type-only imports, side-effect imports (`import "./x"`), re-exports, augmentations and `import x = require()` order the check: the importer waits for the file it imports.

A dynamic `import()` is the exception. It finds its file, which is checked with the rest, but does not order it. The edge is kept apart: `ModuleGraph::dynamic_dependents` lists the files that load a file this way, separate from `dependents`, so each kind is in exactly one of the two lists and nothing that orders by `dependents` can pick up an edge that must not order. What has to rerun when a file changes is `dependents` together with `dynamic_dependents`.

Two tests pin the rule. `a_dynamic_import_finds_the_file_but_does_not_order_it` (`tests/project_imports.rs`) pins that a dynamic import checks the file without ordering it. `only_a_dynamic_import_is_kept_out_of_the_ordering_and_is_listed_apart` (`tests/module_resolution.rs`) pins the rule for dynamic imports, side-effect imports, re-exports and import-equals. Nothing pins one file importing the same file both ways, which lists the importer in `dependents` and in `dynamic_dependents`.

A dynamic import whose target can be read without closing a cycle is read after all; see [library-and-ambient-declarations](library-and-ambient-declarations.md).

**A removed type.** `ModuleTopology`, a second view of the graph, was called only from tests and benches, and the scheduler orders by `ModuleGraph::sccs`. It was removed together with the `petgraph` dependency it needed.

## 3. A cycle is one unit

**Problem.** Files in a cycle import each other, so none can be checked first. The first pass over a cycle finds nothing among its members: an import from a sibling leaves the name unknown.

**Options.** Give the component one shared arena, so its members see each other's types directly. Or keep one arena per file and repeat the check until what the members export stops changing.

**Picked.** Repeat. The shared arena would change how types are stored, how recursive references are held and how declarations are resolved, which is the type system's design and not the project driver's. It belongs with the type-system work, and doing it here would bundle a scheduler change with a rewrite of the arena. The component is the unit of scheduling, of settlement, of publication and of failure isolation; how its types are stored inside is an implementation detail that can change later without touching the driver.

**How it works.** Each round checks every member against one snapshot of what the previous round exported, so the result does not depend on thread timing. A round ends by comparing a fingerprint of the members' exports with the one before. It stops when the fingerprint is stable or when the bound runs out. `cycle_round_cap` gives a cycle of `n` members `n + 1` rounds, and never fewer than four (`MIN_CYCLE_ROUNDS`): an export can travel only one re-export edge per snapshot, so a name declared near one end of a ring of `n` files may need `n` rounds to reach the other end, and one more to see that nothing changed. `settle_cycle` returns whether the exports settled.

**Cost.** The bound is a safety limit and not a proof that every cycle converges. A cycle of inferred signatures can in principle keep changing.

## 4. The contract a cycle must keep

Each condition has a test (`tests/scc_contract.rs`, `tests/circular_returns.rs`, `tests/project_imports.rs`).

| Condition | State |
| --- | --- |
| Each cycle settles independently of the others in its layer | Done: members are grouped by component, each with its own bound (`two_unrelated_cycles_in_one_layer_are_settled_on_their_own`) |
| Convergence follows published results, not internal ids or execution order | Pinned: the outcome does not change with the order of the entries |
| A cycle that does not converge gives an explicitly incomplete result, and the last round is not published as authoritative | Done: its files carry `CycleDidNotSettle`, so `--partial` lists and counts them and their exports are flagged partial. The last round does not report a false missing name, because a partial list keeps it quiet |
| An incomplete cycle does not make downstream files report misleading missing exports | Held already, and tested. An importer keeps reporting its own unrelated errors |
| One cycle that does not converge does not mark an unrelated cycle incomplete | Done, with the first condition |
| A circle of inferred return types is detected and not left to degrade to `any` | Done for functions, see section 5 |

An unsettled cycle never reports a missing name or module among its own members. A file of such a cycle can keep an unresolved-annotation warning when the bound is forced very low (`an_unsettled_cycle_reports_no_missing_name_among_its_own_members`).

## 5. Circular inferred returns

**Problem.** Two functions with no return annotation whose return types are read from each other have no answer. The placeholder `any` they got passed as an answer, with no error. tsc reports TS7023 on each function that is on the circle.

**Picked, within one file.** A call by name to a function declared in the same file, inside a `return` expression of an unannotated function, is noted as a dependency. A cycle among the notes is reported once on each function on it, as `TSR1404`. Recursion inside a larger return expression (`return n * f(n - 1)`) and mutual recursion are reported. A bare `return f(...)` inside `f` is not, because tsc leaves it out of the return type; that call still types as the placeholder `any`. Nothing else is read as a dependency: a call outside a `return`, a call inside a nested function, an annotated function.

**Picked, across files.** A single file sees only its own calls, so each file read the other's function as `any` in silence. Every file now hands over, for each exported unannotated function, the imported functions its return expressions reach (through its own functions too). The nodes are those functions and an edge is such a read. A component of more than one function is a circle, and each member is reported where it is declared, unless its own file already reported it as part of a circle of its own. The result is sorted at once, so the order of the map does not reach the report.

**Cost.** A circle through a method or a variable is not seen and still reads as `any`. A function that is not exported, though it lies on a cross-file circle, is not a node and is not reported, where tsc reports it. Closing that needs inference on demand, which is the later design.

## 6. What is still not done

- A circle across files that passes through a function that is not exported.
- A circle through a method or a variable.
- A shared arena for a component, if the type-system work ever makes it worthwhile. The contract in section 4 is what it would have to keep.

## 7. Invariants to keep

1. **The component is the unit.** Scheduling, settlement, publication and failure isolation are all per component.
2. **A round reads one snapshot.** Every member of a round is checked against the previous round's published exports, never against a neighbour's work in progress.
3. **Not settled means not authoritative.** A cycle that hit its bound is marked incomplete, and the last round is never published as final.
4. **A dynamic import never orders.** The edge is kept and listed apart, and what reruns on a change is the union of both lists.
