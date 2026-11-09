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
| 2026-11-06 | DOM-family lib files load only when the project can observe them | `fe1d6bf` | `lib.dom.d.ts` is 84% of the default library's bytes and most of its time |
| 2026-11-07 | An opt-in cache of checked library files, then of every dependency declaration file, validated by what each asked | `b6102db`, `116a394`, `9263f3a`, `8e32f56` | Checking the same unchanged files on every run was most of a small project's time |
| 2026-11-07 | The import scan of dependency files is cached too | `b0efa6a` | Finding what a file imports means parsing it, 70% of graph building once checking is cached |
| 2026-11-07 | Discovery validates UTF-8 with `str::from_utf8` first | `fdbdb13` | The lossy conversion was half of a warm graph build |
| 2026-11-08 | A cache hit is moved out of its entry; declarations are hashed once per run | `8680d79` | Copying a decoded entry and rehashing shared declarations were the largest parts left of replay |
| 2026-11-08 | Discovery loads each round of files in parallel | `b25da6f` | One core read, hashed and parsed every file in turn |
| 2026-11-08 | The error-checking pass is skipped for dependency declaration files that cannot need it | `2badb7c` | Cold: 60 of 167 ms of `lib.dom.d.ts`, and its errors are never reported |
| 2026-11-09 | Whether the standard-library registry applies is worked out once per project | this patch | One kind of cache replay question was 80% of replay time |
| 2026-11-09 | The library cache is on by default in the command line tool, private, bounded and optional | this patch | A cache that has to be asked for is a cache most runs do not have |

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

- Real generic library shapes through the provider, which needs generic inference. The lib files are loaded and checked (section 9), but a generic method of `Array` or `Promise` is still typed loosely where inference cannot carry it.
- A cheaper second pass for projects whose scripts use each other's names. The corpus cost above is the number to beat.
- A generic interface merged across files, and the printing of merged interfaces.
- `typeof import("x").A.b` and a dynamic specifier that is not a plain string.
- An open observation (2026-10-25): in one of three full-suite runs, `a_very_long_alias_chain_is_cut_off_and_reported_once` in `crates/tsox-cli/tests/depth_guard.rs` got no nesting diagnostic. It passed alone three times and in the other full runs. It is not explained.

## 9. Loading and caching the standard library

This section is the account of how a project's standard library is chosen, checked and kept, and of what was measured and rejected on the way. Numbers are from a single core, release build; `lib.dom.d.ts` is 2.3 MB.

**What loads.** The set comes from `lib` or the target's default (`StandardLibrarySet`). The DOM-family files (`dom`, `dom.iterable`, `dom.asynciterable`, `webworker.importscripts`, `scripthost`) are left out of the first graph, and the project is read once (`project_needs_dom_libraries`) for any global only those files declare, `globalThis`, a script that declares such a name, `declare global`, or a file that does not parse. Any of these loads them; `--eager-libs` or `TSOX_EAGER_LIBS` always does. The test is one-sided: what it cannot rule out counts as a need.

**What a unit's output depends on.** Its text, its options, and the answers its `ImportSource` gave. Nothing else reaches the checker. `RecordingImports` keeps each distinct question with a fingerprint of the answer (a declaration's whole content, export keys, module state, global names, the flags), and a stored output is used only if the project gives the same answer to every question as it did then. That covers questions answered with "nothing". An entry holds up to six outputs, each with its questions; a hit replays them against a view that records nothing and reports the reads a fresh check would. `tests/probe_log.rs` fails if a method is added to `ImportSource` or `DeclProvider` without being added to the recorder.

**What is kept, and where.** The cache keeps, per bundled library file and per dependency declaration file, its unit entry and the import scan used to build the graph. Keys hash the file's bytes and name and the identity of the running program (its size, modification time and path), so a rebuilt checker never reads an older entry. Entries are read strictly and a damaged one is a miss. `FingerprintMemo` hashes each declaration once per `Project`, keyed by the address of its `Arc`; it holds a strong reference to each, so the address cannot be reused while the entry exists.

**It is on by default in the command line tool**, in a per-user directory: `$XDG_CACHE_HOME/tsox/lib-cache` or `~/.cache/tsox/lib-cache` on Linux, `~/Library/Caches/tsox/lib-cache` on macOS, `%LOCALAPPDATA%\tsox\lib-cache` on Windows (`crates/tsox-cli/src/cache_dir.rs`). `--no-lib-cache` switches it off for a run, `TSOX_LIB_CACHE_DIR=off` for a shell, `--lib-cache <dir>` or `TSOX_LIB_CACHE_DIR=<dir>` names another directory, and the flag beats the variable. A directory that was asked for and cannot be used is an error; the default one that cannot be used is not, and the run goes on as if it were off. The library crate installs no cache unless its caller does (`install_library_unit_cache`), and a build without the `snapshot` feature has none. A default that writes to disk has to be private, bounded and optional, and is each:
- *Private.* The default directory is created with mode 0700 and must be owned by the running user (a file the process makes in it has that owner, which is how this is checked without asking for the user id); a looser mode on a directory we own is tightened. What is read back is data about types that the checker believes, so a directory another user could have written to is refused.
- *Bounded.* Entries older than 30 days go, and past 256 MiB the oldest go first, as do the temporary files of writers that stopped half-way. A sweep reads the metadata of every entry, which costs more than a small project's whole check, so it runs at most once an hour (the time of the last is the modification time of `.swept`).
- *Optional.* The three switches above. The workspace's own `.cargo/config.toml` sets `TSOX_LIB_CACHE_DIR=off` so that running the tests never reads or writes a real per-user cache; the tests that need it on set their own environment.

**What the cold cost is.** Per phase for `lib.dom.d.ts`, 167 ms: parse 11, semantic 6, declare 35, bodies 60, exports 55. The bodies pass only finds errors, and a dependency's errors are never reported, so `UnitContext::without_body_checks` leaves it out for dependency declaration files. The checker honors the ask only when the file has no initializer on a variable, class property or enum member, no `export default` of an expression, no `export =` of anything but a name, and no base class that is not a plain name. The equivalence test found the one case that needs the pass: an unannotated ambient `const` (`declare const answer = 42`) is typed there and read when exports are collected.

**Measured at `9b8b21b`, one core, medians of 7 runs (15 for warm).** Output of 108 project configurations was identical to the unmodified tip in every one: uncached, with the default cache cold, and with it warm. The machine's timings vary by about a third from one run to the next, so the ratios are the claim and the milliseconds are a guide.

| Project | Before, no cache | Now, default cache warm |
| --- | --- | --- |
| Plain code | 17 ms | 5 ms (4x) |
| React and `@types/react` | 455 ms | 39 ms (12x) |
| `@types/node` (`types: ["node"]`) | 456 ms | 74 ms (6x) |
| React, with `types: ["node", "react"]` | 689 ms | 92 ms (7x) |

A cold first run costs what the cache-free run costs, less the skipped error pass (see above): React 371 to 263 ms on a quiet machine.

**A profile of the warm Node types project** (467 unit lookups for 185 files, every one a hit, at most five stored variants per file; the import cycle is settled over several rounds, and the second pass looks every file up again) split the replay by kind of question. One kind, `requires_registered_global_types`, took 23.6 ms of the roughly 29 ms of replay, 44 microseconds a question against under one for every other kind. It asks whether the standard-library registry applies, which depends on the graph and its options and on nothing about the file, and it was worked out once per `ImportsOf` instance. The replay view is a new instance per lookup, so it was worked out 539 times, and the check itself worked it out once per file. It is now worked out once per `Project`. That took the warm Node types project from 92 to 68 ms and the plain project from 9 to 5 ms, and it makes a run with no cache a little faster too. Sharing a decoded entry between rounds, which the first reading of the profile suggested, would have saved about 12 ms at most and was not built.

**Tried and not done.**
- *Building only the declarations a project reaches.* The DOM unit costs the same with and without declaration bodies (170 to 185 ms either way), so a selective build saves nothing; the time is in the declare, bodies and export passes over the file. Skipping those per declaration would need a lazy declare pass in the checker.
- *A snapshot shipped with the binary.* Its key would have to name the checker's code, and a stale one would be silently wrong or silently unused. The cache above is written by the program that reads it.
- *Caching a project's own files.* They change constantly, so an entry would be written and read once.
- *Sharing a decoded cache entry between the rounds of a cycle.* Decoding is 12 ms of the warm Node types project, and sharing means copying a hit instead of moving it out, which gives back part of that. The registry question above was the larger cost by far.

**Parallel discovery.** Each round of newly found files is loaded on the checking pool (big stacks, stack guard set) and then resolved one file at a time in id order. The published ids follow path order, so interning order cannot show in the result. One core takes the sequential path. Its speedup has not been measured: the machine it was written on has one core.

## 10. Invariants to keep

1. **The answer does not depend on file order.** Every table is built in path order, and each feature has a test that reverses the listing.
2. **A name that cannot be typed becomes loose, not wrong.** An `any` that loses an error is better than a type that invents one.
3. **A global is visible only where it is unresolved.** A file's own declarations win.
4. **An augmentation never stands in for a declaration it does not know in full.**
5. **A lazy import is read only when waiting for it cannot close a cycle.** Otherwise it stays unread.
6. **A stored result is used only if everything it depended on is still true.** The questions a unit put to the project, with fingerprints of the answers, are the whole dependency. A cache never answers for a project it was not made for.
7. **A cache never makes a run different, only faster.** Output with the cache cold, warm or off is identical, and a damaged or foreign entry is a miss.
8. **Skipping work is allowed only where the equivalence test covers what the work contributes.** `tests/declaration_units_without_bodies.rs` runs each declaration file both ways; a new construct that can need the skipped pass belongs in `declarations_need_the_pass` first.
9. **A default that writes to disk is private, bounded and optional.** The cache directory is the user's alone, has a size and an age limit, and has three ways to switch it off; a default that cannot be used is skipped, never an error.
10. **Whatever a replay or a check asks more than once is worked out once.** A question whose answer does not depend on the file belongs to the project. The profile that found this was by kind of question, so a new kind of question gets its own line in it.
