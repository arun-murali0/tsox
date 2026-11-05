# Cross-File Types: Keys, Portable Declarations, Units, Imports and Exports

> **Architecture case study:** letting a type written in one file be used in another when every file is checked in its own arena, and when a wrong answer is worse than a missing one.

[module-resolution](module-resolution.md) hands the checker files in a safe order. This document is what happens once the checker uses that order: how a declaration gets an identity that survives a run, how it moves between arenas, how a file's exports reach the files that import it, and where the first plan and the code differ.

## 1. How it evolved

| When | Change | Commit | Why |
| --- | --- | --- | --- |
| 2026-10-03 | Module graph, layers, parallel project check | `5f2d201` | Files must be checked in an order where a dependency comes first |
| 2026-10-11 | Stable keys and portable declarations | `45b6098` | Ids are run-local; a declaration has to carry an identity and a shape that do not depend on one arena |
| 2026-10-11 | A unit's annotated exports are collected | `78ea2cd` | `check_unit` returned an empty export list because nothing read the arena before it was cleared |
| 2026-10-11 | `Worker`, `check_unit`, cancellation, panic containment | `d3662d7` | A file is now a unit of work with an owned output |
| 2026-10-11 | Imports are bound to the dependency's real exports | `7327352` | The pieces for cross-file types existed and nothing used them |
| 2026-10-11 | `check_project` passes each layer's exports to the next | `7183c80` | Every file was still checked on its own, so an import was never looked up in the file it names |
| 2026-10-12 | Imports made honest: diagnostics, hidden and inferred exports, `export *`, cycles | `cacdbdb` | A typo'd import bound nothing and reported nothing; several exports were silently dropped |

## 2. What was planned

The design set the shape before any of it was built:

- **Identity.** A module has a key that does not depend on the run, and a declaration has a `DeclKey`. The shape of a declaration is a separate `SigHash`, so editing a body does not change the key of everything that refers to it. Hashes are XXH3-128 over an explicit byte stream whose first byte is a format version. (See [decisions](../decisions.md).)
- **Moving a type.** A declaration crosses arenas as plain data, and an importer rebuilds it. No arena is shared between files.
- **The unit.** A unit is one strongly connected component of the module graph, checked by one worker in one arena in three phases: declare, shapes, bodies. Members of a cycle share the arena, so nothing crosses arenas inside a cycle.
- **Inferred exports.** An unannotated export such as `export const x = f()` is resolved on demand through its slot, and a re-entry of a slot already being resolved is the circularity error tsc reports.

## 3. Identity that survives a run

**Problem.** `FileId` is a place in the order for one run, and a `TypeId` is an index into one arena. Neither can be stored, hashed into a key, or compared between two arenas, and a cross-file reference has to name something that is the same in both.

**Choice.** Three keys, all XXH3-128 over an explicit byte stream (not `#[derive(Hash)]`, whose layout is not a stable format):

- `ModuleKey`: a project-relative path with `/` separators, or a package name, version and path inside it, or an absolute canonical path for anything else. Nothing in it comes from a `FileId`, the absolute root or a timestamp.
- `DeclKey`: the module, the local name and the position in a merge group. A reference from one declaration to another carries this key and nothing else.
- `SigHash`: the declaration's own public shape. Other declarations appear in it by `DeclKey`, so a body-only edit leaves it unchanged. That is what lets a dependent stop re-checking when only a body changed.

The first byte of every stream is `ENC_VERSION`, so a change of encoding invalidates stored fingerprints instead of mixing two formats. Type hashing ignores creation order, union order, property order and parameter names; an intersection keeps its member order, because order is its identity ([intersections](intersections.md)).

**Gained.** Keys are stable across runs and when a project is moved. `xxhash-rust` stopped being optional so keys work without the `module-resolution` feature. The benchmark run of this commit flagged none of the 14 benchmarks as regressed, with instruction counts between +0.1% and +3.2%. The new code is not on the `check_source` path, so that was read as baseline or layout noise, and it is still to be confirmed against a baseline saved before the commit.

**Cost.** Two copies of one package name and version with different contents, one patched in place, share a key. It is accepted and written down in [decisions](../decisions.md).

## 4. A declaration as data

**Problem.** An import has to rebuild, in the importer's arena, the type the exporter's arena holds, and a declaration can refer to itself or to others.

**Choice.** `PortableDecl` holds a declaration as arena-free data, and `export_decl` returns `None` for anything that has no stable key, so an unsupported export makes a unit incomplete and never wrong. `Importer` rebuilds a portable declaration in another arena and shares one slot per key. A declaration that is missing imports as the error type, which does not cascade into false diagnostics.

**Cost, found while using it.** An imported generic has no AST to read its parameter list from. The namespace keeps the parameters (node, bound, default) next to the entry, and the type reference code builds its bindings from that one record, so arity, defaults, bound checks and the `Pair<A, B>` display all read it. That needed a default in the portable form: `PTypeParam` gained one, it became part of the signature hash, and `ENC_VERSION` moved to 2 (the golden hashes were regenerated for that reason only).

## 5. The unit, and what it exports

A file is a unit with an owned input (`UnitContext`: its module key and an `ImportSource`) and an owned output (`UnitOutput`):

- `diagnostics`, and `incomplete`, set when something exported could not be carried;
- `exports`, the exported declarations as portable data, and `export_names`, the name each is exported under;
- `exports_complete`, true only when the collector ran and left nothing out.

The check is split into declare, bodies and publish, so a unit's imports can be bound before its own declarations are declared. A `Worker` per rayon thread clears its arena after every unit. Cancellation is checked when a unit starts and while it runs, and a panic is contained to the unit that raised it: it produces one internal-error diagnostic and an output with no exports and an incomplete list, so a dependent reads the file's exports as missing, not as empty, and reports nothing against them.

**Collecting exports grew in three steps**, each triggered by a case the last one dropped:

1. `78ea2cd` exported only annotated interfaces, aliases, functions and variables. Anything else (classes, enums, default and re-exports, overloads, a body naming an unexported declaration) was left out and set `incomplete`.
2. `7327352` lists top-level declarations first, each with the names it is exported under. That covers `export default`, `export { a, b as c }` for declarations written anywhere in the file, and passing an import on (`export { X }`, `export { X } from "./a"`) under the key it already has.
3. `cacdbdb` registers every interface and alias with the resolver, exported or not. One that an export reaches leaves the unit under its own key and no name, found by walking the references of what already left, so `interface Hidden` plus `export function f(h: Hidden)` is complete and the signature crosses files, and the hidden one cannot be imported by name. Exports are read after the bodies are checked, so an unannotated `const`, `let`, `var` or function is exported with the type that was inferred (a `const` keeps its literal, a `let` is widened, a return type is the body's). `export *` carries the other module's names under the keys they already have, never `default`; a name the file exports itself wins, the first star wins over a later one, and a star over a partial or unknown module makes the barrel partial.

## 6. Passing exports between files

**Problem.** `check_project` checked every file on its own, so an import was never looked up in the file it names.

**Choice.** An import is resolved through the graph's own edges, by the specifier as written, so the resolution the graph already did is the one used. What a layer exports is collected before the next layer starts, and every file of a layer reads the snapshot taken before the layer began. The report does not depend on how the threads interleave.

**Gained.** The first fixture from the plan held end to end: `a.ts` exports an interface, `b.ts` misuses it, and the command line reports TS2322 at the right position in `b.ts`. The tests are `tests/unit_exports.rs` (37), `tests/unit_imports.rs` (36), `tests/project_imports.rs` (29) and `tests/import_diagnostics.rs` (10).

**Cost.** An import of a file that is not checked (a dependency's declaration file) or not there binds nothing. That loses an error and never invents one.

## 7. Import diagnostics: report only against a complete list

**Problem.** A typo in an import bound nothing and reported nothing, which is the checker looking clean for exactly the code it did not read.

**Choice.** A relative specifier that no file answers is TS2307. A name a module does not export is TS2305, and a default import of a module with no default is TS1192. They are reported only against a module whose export list is complete (`exports_complete`), so a name missing from a partial list, such as a class or an `export *` over an unknown module, is never reported. `ImportSource::module_exports` says whether a specifier names no file, a file whose exports were not read, or a checked file and whether its list is complete; the default implementation knows nothing, so existing implementors keep compiling. Bare specifiers are not reported until packages are read.

**Cost.** A real missing export from a module the checker only partly reads goes unreported. That is rule 1 of [case-study](../case-study.md), applied.

## 8. Cycles: where the code differs from the plan

**Plan.** One strongly connected component is one unit in one arena, so members of a cycle see each other directly.

**First built.** A unit was one file, and the members of a cycle were separate units that shared a layer. The first pass over a cycle therefore found nothing among its members: an import from a sibling left the name unknown. `cacdbdb` checked the members again, each round reading the snapshot the previous round left, until what they export stops changing, at most four rounds. A file that merely imports a cycle sees what the cycle exports.

**Since then.** The component became the unit of scheduling, settlement, publication and failure isolation, with each member still in an arena of its own, and the round bound grows with the cycle (one round per member plus a spare, never fewer than four). Repeating the check was chosen over a shared arena because a shared arena would change how types are stored, which is the type system's design and not the project driver's. [cycles-and-settlement](cycles-and-settlement.md) tells that story, and [decisions](../decisions.md) records the rule.

**Gained.** Imports among cycle members find what the previous round exported, and the report is the same however the threads are scheduled.

**Cost.** Extra checks of every member of a cycle. An export set that has not settled when the bound runs out is marked incomplete (`CycleDidNotSettle`) and is not published as final.

## 9. What is still not done

Classes, enums, overloaded and generic functions, default exports, `export * as ns`, installed packages and path aliases all cross files now; the [language coverage](../language-coverage.md) page measures them against `tsc`. What remains:

- **A declaration the collector cannot represent yet** marks the export list partial, with a named reason. The list of reasons and their counts is in [partial-results](partial-results.md); one, `UnrepresentableDeclaration`, accounts for most of them.
- **On-demand inferred exports.** The design resolves an inferred export through its slot when it is asked for. The code reads inferred types after the bodies are checked instead, so a circle of inferred return types that crosses a function that is not exported is not reported (see [cycles-and-settlement](cycles-and-settlement.md)).
- **Benchmarks against a saved baseline** for the keys commit.

## 10. Invariants to keep

1. A key never contains a `FileId`, an absolute root or a timestamp.
2. A change of encoding bumps `ENC_VERSION`; golden hashes are regenerated for that and nothing else.
3. Anything the portable form cannot carry sets `incomplete`, and a dependent reports a missing name only against a complete list.
4. Every file of a layer reads the same snapshot, and so does every round of a cycle.
5. A missing or unread module binds nothing; it never binds something wrong.

The architecture lesson is: **when a fact cannot be carried across, record that it was left out, and let the other side report less, never something different.**
