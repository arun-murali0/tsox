# Partial Results: Saying What the Export Model Cannot Say Yet

> **Architecture case study:** what a checker does when the list of names a file exports is known only in part, why a partial list has to be safe, and why the way to improve it is to measure root causes and not to count affected files.

[cross-file-types](cross-file-types.md) describes how a file's exports travel to the files that import it. That model does not cover every TypeScript declaration. This document is about the gap: how the checker records that a list is incomplete, why that record makes the checker quiet and not wrong, and how the gap is measured so the next piece of the export model is chosen by evidence.

## 1. How it evolved

| When | Change | Commit | Why |
| --- | --- | --- | --- |
| 2026-10-16 | One reason for each way the collector gives up, and a test for each | `89a0621` | "This list is partial" says nothing about what to build next |
| 2026-10-18 | Partial exports summarized by reason and by how far each reaches | `f16f2f5` | One declaration that cannot be exported makes every file that re-exports from it partial, so a flat list reads as a dozen problems when it is one |
| 2026-10-19 | The summary is wired into the library and the command line | `062a9cb` | `tsox --partial` listed files and reasons and gave no count of the files affected |
| 2026-10-19 | A file in a cycle that did not settle is partial with its own reason | `1fe24cf` | See [cycles-and-settlement](cycles-and-settlement.md) |

## 2. A partial list is safe by construction

**Problem.** An export list can be incomplete for many reasons: a declaration kind the portable form does not carry, a pattern that binds no name, a module that could not be read. If the checker treated an incomplete list as complete, every name it left out would be reported as a missing export in every file that imports it. Those errors are false, and false errors are worse than missing ones.

**Picked.** A list carries a set of reasons. A file that imports from a list that is partial stays quiet about a name missing from it. The name is unresolved silently, and an importer keeps reporting its own unrelated errors. A partial list therefore costs completeness and never correctness, and that is why the export model can be extended one declaration kind at a time.

**Cost.** A real missing export is not reported while the list it should have been in is partial.

## 3. Fifteen reasons, three kinds

`IncompleteReason` (in `crates/tsox-checker/src/bridge/exports.rs`) names each way the collector gives up. `ReasonKind` (in `crates/tsox-project/src/module_resolution/partial.rs`) says whether a reason starts a partial list or only passes one on.

| Kind | Reasons | Meaning |
| --- | --- | --- |
| Root cause (12) | `CheckerCrashed`, `UnsupportedDeclaration`, `EmptyBindingPattern`, `ShorthandAmbientModule`, `AnonymousDefaultWithoutSignature`, `NonObjectExportAssignment`, `UnknownLocalExport`, `DeclarationFileWithoutExportStatement`, `UnrepresentableDeclaration`, `StarFromUnknownModule`, `NamespaceStarFromUnknownModule`, `CycleDidNotSettle` | The collector gave up on something in the file itself |
| Mixed (1) | `UnresolvedReExport` | A re-export of a name that cannot be found: the module is missing, the module does not export the name, or its own list is partial. The reason does not say which |
| Propagation (2) | `StarFromPartialModule`, `NamespaceStarFromPartialModule` | The file is partial only because a module it re-exports with `export *` is |

`export *` from a module that is missing, or was never read, is a root cause of the file doing the re-export: the cause is the module, not a partial list upstream.

**The reason with the thinnest test.** `UnresolvedReExport` is reachable only for erroneous programs: `export { a } from "./a"` where `a.ts` does not export `a` (tsc's TS2305), or from a module that does not resolve (TS2307). It needs two fixtures and stays.

**A reason that was removed.** `UnsupportedDefaultExport` was dropped. With the pinned Oxc 0.151 its only producer, the catch-all arm of the `export default` match, cannot be reached, so that arm marks `UnsupportedDeclaration`.

## 4. Measure root causes, then reach

**Problem.** The first report listed each partial file with its reasons and did not say how many files it affects. One incomplete declaration can pass through a large graph, and counting affected files makes a single root cause look like dozens of separate problems.

**Picked.** Count root causes first and report propagation apart. `tsox --partial` prints one row for each reason with:

- the files whose list is partial for that reason (a file with two reasons is in two rows);
- the project files that import one of them directly;
- the project files that reach one of them through other files.

**Cost.** A dependent here is any file that imports a partial one, whether or not it reads a name that is missing. The figure is a reach, not a count of wrong answers. A dynamic `import()` is not a dependency edge, so a file that reaches a partial one only that way is not counted.

## 5. What the measurements said

| Project | Files | Partial | By reason |
| --- | --- | --- | --- |
| The TypeScript test corpus as one project | 10,083 | 518 | 449 are `UnrepresentableDeclaration` |
| Zustand at its default branch, no `node_modules` (2026-10-26) | 34 | 14 | 11 `UnrepresentableDeclaration`, 1 `DeclarationFileWithoutExportStatement`, 1 `UnresolvedReExport`, 1 passed on by `export *` |

On Zustand there were no cycles and 28 unresolved imports. The project is clean under `tsc`, yet tsox reported 127 errors: 89 TS2345, 23 TS2307 (from the missing packages), 8 TS2339, 3 TS2322 and 5 others. The rest are false positives to triage; one example is a type parameter whose constraint does not resolve, which loses its members (`S` in `src/react.ts`). The lesson is the one the measurement was meant to give: on both projects a single reason, `UnrepresentableDeclaration`, accounts for most of the partial files (449 of 518, and 11 of 14), so extending the export model there comes before anything else.

## 6. Declaration files without export statements

A declaration file with no `export` statement is not automatically complete. In tsc such a file may be a global script, an external module whose declarations are exported implicitly, or a file of ambient module declarations, and which names are visible depends on which it is. The checker cannot tell them apart from the file alone, so today these files stay partial under `DeclarationFileWithoutExportStatement`. tsc also reports TS2459 for a name a `.d.ts` file declares locally and does not export, and tsox has no such code.

## 7. What is still not done

- More real-project `--partial` runs. One real project has been measured, and the corpus is a stress case and not a real project.
- Triage of the Zustand false positives.
- A semantics for declaration files without export statements, so they can be complete.
- A distinction in the report between the three causes behind `UnresolvedReExport`.

## 8. Invariants to keep

1. **Partial never produces an error.** Code that reads an export list stays quiet about a name missing from a partial one.
2. **Every way of giving up has a reason.** A new `return` of "not supported" in the collector adds a reason and a test that reaches it.
3. **Roots before propagation.** Counts and reports lead with root causes, so one cause is not counted as many.
4. **Each reason has a test.** `crates/tsox-cli/tests/partial_reasons.rs` gives each reason a source that produces it and nothing else, and the CLI prints the reasons with `--partial`.
