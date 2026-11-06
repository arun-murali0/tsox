# Library and Ambient Declarations: Names That Are Not Imported

> **Architecture case study:** how a project that declares globals, augments modules, loads types by configuration and imports lazily can be checked in any file order with the same answer, and why a checker without a real standard library types a method loosely and not wrongly.

Most of a file's names come from its own declarations and its imports. Some do not: a global declared in another file, a member added to a module by an augmentation, an `Array` method, a type loaded because `tsconfig.json` names it. Each of these is a name that is visible without an import, which makes it a dependency the graph does not see. This document is about how those names reach the checker, and about what it costs when they do.

## 1. How it evolved

| When | Change | Commit | Why |
| --- | --- | --- | --- |
| 2026-10-22 | Global declarations are visible across the project | `adc9af1` | A global declared in one file was an unknown name in every other |
| 2026-10-23 | Arrays and Promises get their library methods | `3e24103` | `arr.map(f)` and `p.then(f)` were false "property does not exist" errors |
| 2026-10-24 | Module augmentation: a hole closed; import types | `4e37ccd` | An augmentation of a package with no module key hid every member the package declared |
| 2026-10-25 | Lazy imports that cannot close a cycle are read; async functions are checked | `ac6c6ad` | `await import("x")` had no type, and an unannotated `async function` was never declared |
| 2026-10-31 | `types`, `typeRoots`, reference directives and project globals | `35e4962`, `36d0bd2` | A package's types come from configuration and from `/// <reference>` as well as from imports |
| 2026-11-03 | The TypeScript 6.0.3 lib files are vendored as data | `0e76352` | The first step toward a real standard library |

## 2. The shape of the problem

**What the checker had.** A list of global names (`lib_global_types.rs`) that stopped the checker reporting `Promise` or `Record` as unknown, and nothing else. It gave no shapes: a `Promise` was a name and an `Array` had only `length`. There was no `declare global`.

**Why order matters.** A global is visible to every file, so a project's result can depend on which file the checker reads first. That is not acceptable (see [scheduler](scheduler.md)): every step below is built so the answer is the same however the files are listed or scheduled, and each has a test that reverses the order.

## 3. Project globals

A top-level declaration in a script file (a file with no `import` or `export`) and a member of `declare global { }` is visible to every file of the project, with its real type.

- Names are published per file, and the project table is built in path order, so the result does not depend on the order files are listed or checked in (tested by reversing the entry list).
- A name declared in several files merges like a module augmentation. A file that declares a global interface itself joins the other files' parts of it, for script files and `declare global` blocks, in either listing order.
- A file binds only the globals it leaves unresolved, so its own declarations win.
- A script lists only its names in the first pass, and is checked again for its declarations only when another file uses one. Two files that declare one global name trigger that second pass for both.

**Evidence.** A script plus a `declare global` block used from a third file went from 5 false TS2304 errors to none, and matches `tsc`; wrong uses are still reported with `tsc`'s codes (`tests/project_globals.rs`). On the corpus as one project the diagnostics went from 25,670 to 25,246 with failures unchanged at 1,103. Not measured: whether those 424 fewer diagnostics are all false errors; compare with `tsc` before relying on it.

**Cost.** A project whose scripts use each other's names needs the second pass. Measured on the corpus: check time about +57 %, peak memory 26.7 to 79.5 MB.

**Limits.** A global that refers to another global is typed from the first pass. Declaration-file providers are checked before globals exist. Global namespaces are skipped. A merged interface prints as an anonymous shape in a message.

## 4. Library members, loosely

**Problem.** Before this step an array had only `length`, so `arr.map(f)`, `arr.push(x)` and `p.then(f)` were false "property does not exist" errors.

**Picked.** Give arrays and Promises their library methods, in two classes.

- A method that only moves or reports elements is typed from the element type: `push`, `slice`, `indexOf`, `join`, `pop` and others. Wrong uses are still rejected, with `tsc`'s codes.
- A method whose real signature needs generic inference (`map`, `reduce`, `flatMap`, `find`, `filter`, `flat`, and the Promise chain) takes callbacks and returns `any`. It loses errors and invents none.

That is the rule behind every loosely typed library member: when the real type cannot be computed yet, the checker uses the type that can be wrong in only one direction. A missing error is acceptable; a false one is not.

**Evidence.** `tests/library_members.rs` (6 tests). On Zustand, which has no errors under `tsc` or `tsgo`, tsox errors fell from 104 to 94 and TS2339 from 18 to 8, and every removed error was false. Corpus diagnostics went from 25,246 to 24,761, failures unchanged, with no change in allocations or peak memory.

**Not done.** Loading a library subset through the project provider so that `Array`, `Promise` and `Record` have real generic shapes. It needs the generic inference the checker did not have when this was written, and the loose typing is the safe stand-in until then. `Record` mismatches and `Promise<T>` assignability are not checked, a Promise chain after the first call is `any`, and `ReadonlyArray`, tuple, `Map` and `Set` members are not added.

**The vendored library.** `crates/tsox-base/lib/typescript-6.0.3/` holds all 108 `lib*.d.ts` files of TypeScript 6.0.3, unmodified, with the order each target loads them in (`<target>.order.json`). They are data: nothing embeds or reads them yet, because the loader decides whether to embed, compress or read them lazily, and embedding 2.8 MB would grow the wasm bundle. The plan is to check them once as a provider module, merge their global interfaces, suppress their diagnostics and load them by name. The fixtures in `tests/fixtures/lib-probe/` are one small program for each construct in these files that the checker cannot resolve yet, and `tests/lib-probe-hangs/` holds the two inputs that still hang. See `crates/tsox-base/lib/README.md`.

## 5. Module augmentation

`declare module "x" { ... }` in a module adds to what `x` exports. The behaviour existed, and this step tested it and closed one hole.

**Tests** (`tests/module_augmentation.rs`, 6): two files augmenting one interface both contribute, with `tsc`'s codes for wrong uses; the result is the same when the file list is reversed; an augmentation can add a function, a constant or a new interface; a package keeps its own members next to an augmentation.

**The hole.** When the augmented module's own exports are not known in full, for example a package file whose `package.json` has no `name` and `version`, so it has no module key, an augmentation of an interface or class used to stand in for the whole declaration. Every member the package really declares was then reported missing, a false TS2353. Now such an augmentation is used only when the module's list already has the name or is known in full. Otherwise the name stays unresolved, silently, and a function or constant the augmentation adds stands alone. The regression test fails without the fix.

**Later additions.** An interface merged into a class reaches `new C()`. An enum augmentation keeps the original members and adds the new ones. A namespace augmentation adds values and types.

**Not done.** A generic interface merged across files keeps only the first declaration's shape. Merged interfaces print as an anonymous shape in messages. Augmenting a module that is only declared ambiently (`declare module "x"` in a script) is a provider, not an augmentation.

## 6. Import types and lazy imports

`typeof import("x")` is the module object, `import("x").Name` is the named type, and `import("x")` is a Promise of the module object, so `await import("x")` gives it. Type arguments (`import("x").Box<number>`) and a qualified `typeof import("x").y` resolve too. The module object is built the way `import * as ns` builds one, and only when the module's exports are known in full.

**The ordering problem.** A lazy `import("x")` can be read only when the check can wait for `x` without making a cycle, and the answer must not depend on thread timing. `ModuleGraph::ordered_dynamic` takes the dynamic edges in path order (not file-id order) and keeps one only if its target does not already reach the importer. The check waits for those units, so the answer is the same in every run. `dependencies` and `dependents` are unchanged. A lazy import that would close a cycle stays unread; of two files that load each other, the first by path is kept.

**Evidence.** `tests/import_types.rs` (9 tests), including the reversed listing order. The lazy-load probe matches `tsc` on all three errors. Corpus diagnostics went from 24,761 to 24,720 with failures unchanged, allocations +0.15 % and peak memory +0.6 %.

**Found on the way.** An `async function` declaration with no return annotation was never declared, and its body was never checked, so errors inside it went unreported and a call to it was unknown. It is declared like a sync function now and returns a Promise of what its body returns (`tests/async_functions.rs`, 5 tests; Zustand errors stayed at 94 and warnings fell from 401 to 327). Unannotated async function expressions in a default export, overload signatures and generators are not covered.

**Not done.** A dotted `typeof import("x").A.b`, and a dynamic specifier that is not a plain string. Both stay unresolved.

## 7. Type roots, `types` and reference directives

**Picked.** `types` and `typeRoots` are applied. The declaration files `types` names are entries of the graph, found in `typeRoots` first and then as ordinary packages (a package subpath goes through `exports`). When `types` is unset it takes nothing, because TypeScript 6.0 changed the default to `[]`; `tsc` 6.0.3 was checked on seven cases and tsox matches all of them. With them loaded, an `@types` directory or a `typeRoots` setting no longer holds back the report of a missing package (see [project-configuration](project-configuration.md)).

`/// <reference path="..." />` and `/// <reference types="..." />` at the top of a file are edges, so the files they name are read and their globals count. This matches `tsc` on a package that references a sibling file and another package. A directive after the first statement is an ordinary comment. An unresolved `reference types` is TS2688 and an unresolved `reference path` is TS6053, judged like an import, only when the graph is sure. `/// <reference lib>` and `no-default-lib` are accepted and ignored, because the checker's library is fixed.

**The second pass, narrowed.** The second pass now checks again only the files that can change: users of a global name or a UMD alias, files that import a specifier an ambient module or augmentation declares, files that read from an augmented module, and everything that reads from those. A wildcard module (`declare module "*.css"`) still checks every file. Unit tests compare the narrowed result with a forced full pass.

**Cost, measured.** On the corpus as one project, check step, against the state before this work: wall time 1,139 to 1,596 ms (+40 %), CPU 1,116 to 1,567 ms, allocation 1,322 to 1,977 MB, peak memory 26.7 to 79.8 MB. Narrowing took about 12 % off the previous step's wall time and changed neither allocations nor peak memory, so on the corpus it still behaves like a full second pass. The cause is not confirmed; the likely one is a wildcard module or a near-total set of users in the corpus. The corpus as one project is a stress case and not a real project.

**Evidence.** `tests/type_roots.rs`, `tests/reference_directives.rs`.

## 8. What is still not done

- Real generic library shapes through the provider, which needs generic inference. The lib files are vendored and nothing reads them.
- A cheaper second pass for projects whose scripts use each other's names. The corpus cost above is the number to beat.
- A generic interface merged across files, and the printing of merged interfaces.
- `typeof import("x").A.b` and a dynamic specifier that is not a plain string.
- An open observation (2026-10-25): in one of three full-suite runs, `a_very_long_alias_chain_is_cut_off_and_reported_once` in `crates/tsox-cli/tests/depth_guard.rs` got no nesting diagnostic. It passed alone three times and in the other full runs. It is not explained.

## 9. Invariants to keep

1. **The answer does not depend on file order.** Every table is built in path order, and each feature has a test that reverses the listing.
2. **A name that cannot be typed becomes loose, not wrong.** An `any` that loses an error is better than a type that invents one.
3. **A global is visible only where it is unresolved.** A file's own declarations win.
4. **An augmentation never stands in for a declaration it does not know in full.**
5. **A lazy import is read only when waiting for it cannot close a cycle.** Otherwise it stays unread.
