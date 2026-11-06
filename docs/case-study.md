# Case study: why the checker is built the way it is

This is the document to read when a design choice looks odd and the question is "why not
the simpler thing?". Each section is one decision: the problem, the options, what was
chosen, and how it was checked. The per-topic documents in `docs/architecture/` (arena, narrowing, generics,
intersections, module resolution, cross-file types, foundation dependencies) hold the
detail and tell each one as plan, issue, change and gain; this one holds the reasoning that
connects them.

## 0. What the project optimises for

Three priorities, in this order, decide most questions below.

1. **A wrong error is worse than a missing one.** A checker that reports a false error
   makes people stop trusting it; one that misses an error is merely incomplete.
   Whenever the checker cannot decide, it keeps the wider type, drops the narrowing, or
   reports nothing. This is why so many handlers say "left alone" for the case they do
   not understand.
2. **Agree with `tsc`, measured, not assumed.** `scripts/ts-diag-tool` runs every fixture
   through real `tsc --strict` and the checker, and CI fails if the number of false
   positives rises above `baseline.json`. Features are judged by that comparison, not by
   whether they look like TypeScript.
3. **Keep the common path cheap.** Arena ids, interning and small inline stacks exist so a
   large file costs little, but a design that is fast and wrong loses to rule 1.

## 1. Types are ids into an arena, not a tree of boxes

**Problem.** Types refer to each other, recursively (`interface Node { next: Node }`), and
the same shape appears thousands of times. A tree of owned boxes cannot express a cycle and
copies shared structure.

**Choice.** One `Vec<Type>`, with `TypeId` an index. Types are shallow: they hold ids of
their parts. Anonymous composites are interned, so two identical `{ a: number }` share an
id and equality is an integer compare. Named types stay unique so each keeps its own name
in messages. A declaration that can refer to itself is a `Ref` to a slot, handed out before
its members are resolved and given its body afterwards (section 15). It began as an empty
placeholder object overwritten in place with `set()`.

**Consequence.** Everything that remembers something about an id (the relation cache, the
parameter-scan memo) must be told when an id changes meaning. That was the cost of
placeholders. Section 4 is how it was paid, and section 15 is how most of it was removed.

## 2. Structural subtyping with a "currently comparing" stack

**Problem.** Recursive types compared against each other never terminate if each step
recurses.

**Choice.** A stack of the (left, right) pairs in progress. Meeting a pair already on the
stack counts as true: the pair is assumed to hold while its own proof is being built, which
is the standard coinductive treatment.

**Why a `SmallVec`.** The stack is almost always a handful of entries. Sixteen inline slots
mean the heap is never touched in normal use, and scanning sixteen integers is cheaper than
hashing. A deep type moves to the heap and still works, which a test forces.

## 3. Narrowing is an overlay on the declared type

**Problem.** A variable has one declared type and a different type on each path.

**Choice.** `SymbolTypeMap` holds declared types and never changes inside a branch.
`NarrowState` is an overlay of `(SymbolId, TypeId)` pairs. A condition yields a pair of
overlays, one per outcome. Mutating the declared type instead would make a narrowing
outlive its branch, which is a silent wrong-answer bug.

**Joins.** Saving and restoring is safe but throws away what a branch learned. The state
after a branch is decided by which paths can fall through (a guard clause leaves the
alternate's state, an `if`/`else` that both continue leaves the join). The join keeps a
symbol only if both paths narrowed it, unioning the two types. Dropping a symbol only ever
loses precision, so the join cannot be unsound, which is why it can be one small function
used for `if`, loops and `a && b` alike.

**The `never` trap.** Narrowing a plain `string` by `x === "a"` once produced `never`.
`never` is assignable to everything, so the bug raised no error at all, and the first
fixtures passed. The fix came with fixtures that assign the narrowed value to an
incompatible type and expect exactly one error: the only observable difference between
`"a"` and `never`. When a bug is silent, the test has to be built to make it loud.

**Where the handlers stop.** Property paths, user-defined predicates, optional chaining and
a join after `switch` are not done, and the narrowing document says why each needs more
than another handler.

## 4. A generation counter, not an invalidation rule

**Problem.** The relation cache keys on a pair of ids. When a placeholder is filled in, an
id's meaning changes and the cached answer is wrong. The first design was a rule: "resolve
declarations before checking bodies, and never query while a placeholder is empty". It held
because nothing broke it yet.

**Choice.** The arena counts changes of meaning (`set` then, `resolve_ref` and `clear` now). The cache records the
generation it was filled under and drops its entries if the arena's has moved. No caller
has to remember anything.

**Principle.** If correctness depends on every caller following a rule, make the data
structure follow it instead. The same reasoning put the `FileId` inside `TypeParameterId`
instead of asking each caller to keep files apart.

## 5. Generic inference

**First candidate wins, widened only by subtyping.** `pair<T>(a: T, b: T)` called as
`pair(1, "x")` is a genuine TypeScript error, not `T = number | string`. So a second
candidate replaces the binding only when it is a supertype, and otherwise the ordinary
argument check reports the mismatch.

**Literals widen, unless the bound asks for them.** `identity(5)` is `number`. But
`T extends "a" | "b"` asks for the narrow type; widening would bind `T` to `string` and
then reject the very argument it came from. A bound that mentions a primitive or literal
keeps the literal, and same-primitive literals combine (`pair(1, 2)` is `1 | 2`).

**An unresolvable explicit type argument stays in position.** Arguments are matched to
parameters by position. Dropping one shifts the rest left onto the wrong parameters and
produces a bogus error on an argument that was fine. It becomes the error type, which is
compatible with everything, so exactly one parameter goes quiet.

**The memo is keyed on a fingerprint and confirmed exactly.** Keying on
`(shape, Vec<bindings>)` would allocate a `Vec` on every lookup, which is a good part of
what the memo exists to save. A 64-bit fingerprint picks the bucket and the stored bindings
are compared before a hit is returned, so a collision costs a comparison and never a wrong
type.

## 6. Declaration collisions: report only what is certainly an error

TypeScript merges some duplicate declarations and rejects others. This checker models only
interface-with-interface merging, so it replaces the earlier entry for the rest. Flagging a
merge it does not model would report an error `tsc` does not. So a collision is reported
only when it is illegal in TypeScript: an alias on either side, two classes, an enum
against anything but an enum. Interface-with-class and enum-with-enum are accepted
silently and recorded in the checklist as a known limit. Both declarations are reported,
as `tsc` does, so the output compares line for line.

## 7. Sessions and file identity

An editor re-checks one file constantly. Reusing the arena's allocation is the obvious
saving, but only if nothing leaks between checks, so `clear()` resets every side table and
a test checks that a name from the previous file is unknown in the next. The reusable state
lives in a `CheckSession`, not in `TypeChecker`: the checker stays free of interior
mutability, and independent sessions can run in parallel. `FileId` is a plain index, not a
path, so type identity never depends on a filesystem.

## 8. The project graph: reuse the resolver, keep every edge

**Problem.** A project is many files, and the checker only knows one. Something has to
decide which file an import means, the order to check in, and which files changed.

**Resolver.** `oxc_resolver` stays the resolver. Writing a second one means owning
`exports` maps, extension probing and `node_modules` lookup for no gain, and the two would
drift. The wrapper picks TypeScript's options (`types` first, NodeNext extension aliases)
and hides the dependency's error type.

**Keep what failed.** An import that does not resolve stays in the graph as an edge with
no target, and a file with a syntax error is flagged instead of reporting zero imports.
Dropping either makes a broken file look like a file with nothing to say. The report
follows the same rule: a file that could not be read or checked is listed as such, never
left out.

**No recursion, no locks.** Cycle detection is Tarjan with an explicit stack, so import
depth is bounded by memory and not by the call stack, and a test builds a 50,000-file
chain. Parallelism is one `CheckSession` per file inside a layer, so nothing mutable is
shared and the checker gained no locks. The report is sorted by `FileId`, which is why
`FileId` is now ordered.

**Ids in path order.** Discovery hands out ids as files are found, so the numbering depends
on the order the entries came in and on how the imports are written. Every list sorted by
`FileId` would then depend on it too. The graph walks with those ids and, when the walk ends,
renumbers the files in path order and remaps every edge. The report, the layers and the
cycles come out the same wherever the walk started, and a test gives one project two entry
orders to prove it. The id is a place in that order for one run, so it is never stored or put
in a key.

**Two-step change detection.** A stat call (length and mtime) answers the common case. A
hash of the bytes is taken only when the stat moved, so a touched file with the same
contents is not a change. The fingerprint is taken before the read, and an unreadable
file counts as changed, because the wrong answer to "unchanged" is a stale result.

**Opt-in.** Everything here reads the filesystem, so it is behind the
`module-resolution` feature and the in-memory and WASM builds do not carry it.

**Fixtures.** The projects live outside `tests/fixtures/` because the tsc comparison
checks each file there alone, where every import would be an error.

## 9. Considered and not done

- **Ignoring parameter names in the intern key.** Two functions that differ only in
  parameter names would share an id, which speeds equality. But diagnostics print
  parameter names (`Expected 2 arguments, missing 'b'`), and a shared id keeps the first
  function's names. Wrong text in a message costs more than the saved comparison, and
  `structurally_equal` already ignores names where equality is asked directly.
- **Caching union de-duplication.** Connecting `structurally_equal_cached` to `alloc_union`
  changed diagnostic counts once and the cause was not found. It stays disconnected.
- **A second resolver.** Resolving a specifier is `oxc_resolver`'s job and it does it
  well, so `ModuleResolver` only chooses options and converts errors. Section 8 has the
  rest.
- **tsconfig `paths` and packages.** The resolver is built without a tsconfig, and an
  import of a file under `node_modules` binds nothing. Cross-file name lookup itself is
  done (section 17); these two are the part of it that is not, not gaps hidden behind the
  graph.
- **Baseline-first benchmarking.** No speed figures are claimed for the reuse work. The
  counters make a regression observable; the numbers belong to a measured run.

## 10. How the decisions were checked

- **Fixtures first, in pairs.** A fixture that must stay clean, and one that must produce
  exactly one error, so both a false positive and a silent over-narrowing show up.
- **The `tsc` comparison.** Every new fixture is run through real `tsc`. A fixture where
  the two disagree is either a recorded gap or a bug, and the false-positive count may not
  rise.
- **Tests for what must not survive.** Reuse code is tested by asserting what is gone (a
  name from the previous file, a cached answer from before a placeholder was filled), not
  by asserting the happy path twice.
- **A long chain for the algorithm that could overflow.** The graph test builds 50,000
  files in a row, because a recursive cycle search passes every smaller test.
- **Counters over opinions.** Hit and miss counts and arena sizes are exposed so a claim
  such as "the memo answers repeats" is a test assertion.

## 11. Unreachable code: cover the container, not the construct

**Problem.** The first walker looked for dead statements by descending into the constructs
it knew: function declarations, `if`, loops, `switch`. A comparison against `tsc` on a
probe set showed the checker never produced a false report but missed one in every
construct the walker had not been taught: arrow bodies, callbacks, class methods,
constructors and getters, `try`/`catch`/`finally`, `for-of`/`for-in`, `do-while`, labels.
On real code those are most of the code, so most of the differences against `tsc` were
this one diagnostic.

**Choice.** Dead code is a property of a statement *list*, so the walker hooks the list
itself through the AST visitor and checks every list the visitor reaches. No construct has
to be named, and a construct added to the language later is covered for free. The control
flow graph still answers "can this statement run", so reachability is not re-derived.

**Constant conditions are the exception, and a deliberately small one.** tsc treats a
literal `true` or `false` condition as known; oxc's graph does not. The walker adds
exactly that rule for literals, and nothing cleverer, so a condition it does not recognise
costs a missed report and never a false one.

**What tsc skips, this skips.** Function, interface and type alias declarations, empty
statements, `var` without an initializer and `const enum`. A dead block is not reported
itself, only its first executable statement, as in tsc.

**Checked how.** 72 fixtures, each compared with `tsc` line for line, split into dead code
in every construct and valid code that must stay silent. The first run of the new walker
disagreed on one fixture, which turned out to be the block rule above.

## 12. Type parameters in scope are fixed, not inferred

**Problem.** Inside `function map<T, U>(v: T, fn: (x: T) => U): U`, the call `fn(v)` was
treated as a call to a generic function: every type parameter found in the callee's type
was inferred, so `U` was never bound and the call returned `unknown`.

**Choice.** The namespace records which declarations the checker is currently inside. A
parameter of such a declaration that appears in a callee's type is bound to itself, which
makes substitution leave it alone. The exception is a call written as the name of a
function or class declaration: that instantiates the declaration afresh even from inside
its own body, so a recursive call and `new Box(x)` inside `Box` still infer.

## 13. Four patterns that were wrong on almost every file

These were found by probing everyday code against the checker, not by reading it, and each
was a false error: an empty array literal where an array is expected (typed `unknown[]`,
now `never[]`), constructor parameter properties (`constructor(public x: T)` declared no
property), a boolean discriminant tested by truthiness (`if (r.ok)`), and the generic
callback call above. The lesson is the one in section 0: the cheapest way to find false
errors is to run the code people actually write, and keep each such case as a fixture.

Array methods (`push`, `map`, `filter`) are the largest false-error source still open,
because only `length` is modelled; it needs generic method typing with callback inference
and is the next piece of work, not a patch.

## 14. Dependencies arrive as pieces, not as a rewrite

**Problem.** The design documents name petgraph, Salsa, Bumpalo and mimalloc. Wiring all
of them into the checker at once means a change nobody can review and a failure nobody
can place.

**Choice.** Each one is a small piece behind its own feature, with its own tests, and
nothing in the checker reads it yet. A default build, the WASM build and the binary CI
compares against tsc do not change. The stages that need a piece start from something
already pinned down.

**Where the design said no.** The topology has no "what does a change reach" search,
because deciding what to recompute is the query layer's job and a second mechanism can
disagree with it. There is no Salsa-interned name type, because the design keeps
types in per-file arenas and not in Salsa-interned ones. The scratchpad takes only `Copy` values, because a bump
never runs destructors. Each of those is a thing the code does not do, written down with
the reason.

**Two answers, one result.** petgraph's cycle search sits next to the graph's own, and a
test holds them to the same answer. The component order is asserted, not assumed.

**Allocator.** mimalloc is set in the binary behind an opt-in feature, since a library
that chose a global allocator would change it for every program that links the crate.

## 15. Declarations as slots, and names on nodes

**Problem.** An interface that mentions itself needs an id before its members exist, so the
arena handed out an empty object and overwrote it in place when the members were done.
That overwrite is why every cache had to be emptied after it, why two empty declarations
compared equal until one was filled in, and why a name could not simply be attached to an
id: naming a shared id would rename every identical type. A pile of workarounds grew
around it: `make_unique` to copy before naming, `duplicate_named` so a memo hit could be
renamed safely, and a check that an argument's name had settled before an instantiation
was reused.

**Choice.** Two changes, one per cause. A declaration is a `Ref` to a slot, and the slot
gets its body when the members are resolved. The `Ref` itself is never rewritten, so
nothing that holds its id has to be patched. And a name is a node: `Named` wraps an alias
or enum, `App` is a generic with its arguments. `get` looks through all of them, so the
rest of the checker keeps matching on objects and never sees a wrapper.

**What went away.** `set`, the empty-object placeholder, the display-name side table,
`make_unique`, `duplicate_named`, `has_settled_display`, and the Record value-type table.
An application's name is written from its arguments when it is printed, so there is no
spelling to go stale; `Record<K, V>` hands back its value type from its own second
argument.

**What did not.** Resolving a `Ref` still moves the arena's generation and empties the
two caches that could hold an answer about it. The planned append-only caches need
relations to expand a `Ref` lazily, and that was left for the unit context.
An `App` still holds its instantiated body, so expansion is eager.

**One behavior change to know about.** A name used to be written onto the id an alias
resolved to. For `type A = Dog`, that renamed `Dog` itself to `A` everywhere. The alias is
now its own wrapper, so `Dog` keeps its name and only uses through `A` print `A`.

**How it was checked.** The full suite, 609 tests before and 615 after, with the existing
fixtures untouched. The unit tests that covered the removed APIs were rewritten for the
guarantee they protected: an alias never renames a shared id, an application is told apart
by its declaration and arguments, and `Box<Node>` prints as `Box<Node>` once `Node` has
its name.

## 16. Intersections: reduce lazily, and keep identity to what an id shows

**Problem.** An intersection can reduce (`string & number`, `{ kind: "a" } & { kind: "b" }`), and the type has to be built while declarations are still being resolved. Reducing at construction time would read a `Ref` that is still an empty object and bake that into the intern table, under an id nothing invalidates: the placeholder bug from section 15, turned around.

**Choice.** Two levels. Construction uses only what an id and its alias wrappers show: kinds, primitives, literals, unions. That part is identity. Anything that needs a body (a discriminant conflict, property merging, which member is callable) is decided on demand, cached per generation, and never written back. tsc does the same, which is why `{ kind: "a" } & { kind: "b" }` is still an intersection that prints as `never`.

**Order is identity.** Members are not sorted, because call signatures are tried in member order and callability needs a body. `A & B` and `B & A` are two ids that are assignable both ways.

**Checked against the real compiler.** The rules were written down only after a probe over tsc 5.9.3, and every fixture was run through it. That caught two things a reading of the docs would not: `void & A` stays an intersection while `null & A` is `never`, and `string & {}` stays while `A & {}` is `A`.

**One thing it found by accident.** Adding the variant to the type enum changed an unrelated test from 5 diagnostics to 1. The four that vanished are false positives that tsc does not report, so something depended on the order of the variants. It was found three days later and was not the order: two literals whose digests collided got a new id on every use, so narrowing, which compares literals by id, narrowed a union to `never`. The intern table now keeps every distinct content at one id (`91454fa`), the test reports 1 whichever place the variant sits in, and the variant still goes last because nothing was gained by moving it.

## 17. Crossing files: keys, portable declarations, units
**Problem.** Every file is checked in its own arena, and a `TypeId` means nothing in another one. An import has to rebuild a type that was built elsewhere, and a typo in an import used to bind nothing and report nothing.

**Choice.** An identity that does not depend on a run (`ModuleKey`, `DeclKey`, and a `SigHash` of the declaration's own shape), a declaration carried as arena-free data and rebuilt by the importer, and a unit of work with an owned input and output. A layer's exports are collected before the next layer starts, and every file of a layer reads the same snapshot, so the report does not depend on thread timing. Whatever the portable form cannot carry sets `incomplete`, and a missing export is reported only against a complete list.

**Where the code differs from the plan.** The plan made a cycle one unit in one arena; the code checks its members again, each round reading the previous snapshot, and the bound grows with the cycle ([cycles-and-settlement](architecture/cycles-and-settlement.md)).

**Cost.** At that point classes, enums and overloads were not exported and packages were not read, so a dependent saw less, never something different. All three cross files now. The full story, with the commits, is [cross-file-types](architecture/cross-file-types.md).

## 18. A literal property is typed against its declared type
**Problem.** `const e: Ev = { tag: 'x', n: 1 }` against `interface Ev { tag: 'x'; n: number }` was reported as `Type 'string' is not assignable to type '"x"'`. An object literal widened every property the moment it was read, so `tag: 'x'` became `tag: string` before it was compared with the declared type. tsc gives no error, and discriminated unions and event types are written this way all the time, so this was a false positive in single-file mode as well as across files.

**Choice.** The object literal is read with the declared type it is checked against. `CheckContext` gets `contextual`, which a caller that has a declared type sets just before inferring the expression and `infer_expression_type` takes on entry, so it can never reach a deeper expression by accident. When the declared type of a property holds a literal (a literal, or a union containing one), the property keeps its literal; otherwise it widens as before. The declared property type is handed down, so nested objects and array elements are read the same way, and a union of objects offers one declaration per member that has the property, which keeps each member's tag of a discriminated union.

**Not covered.** Generic parameters (a generic parameter declares no literal), tuples, and spreads. Commit `2910ce5`, with `tests/contextual_literals.rs`.

## 19. Rules of thumb that fell out of this

1. If a wrong answer is silent, write the test that makes it loud.
2. Prefer the data structure that cannot be misused to the rule that must be remembered.
3. Dropping information is safe; inventing it is not. Joins, narrowing and inference all
   resolve doubt toward the wider type.
4. Report an error only when sure the other checker would too.
5. Keep a deliberate non-decision written down, with the reason, so it is not reopened by
   accident.
6. A fact that could not be resolved is data. Keep it where a later phase can report it.
7. A fact that cannot be carried across a boundary is recorded as left out, so the other side reports less and never something different.
