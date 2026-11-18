# tsox remaining work: gap register, solver design and checklist

> **Snapshot.** File names in this document (`narrow.rs`, `exports.rs`, `declare.rs`, `calls.rs`, `generics.rs`) are the ones before the splits; they are directories now. `new-architecture-progress.md` has the current layout and status.

Everything still open outside the flow work and the semantic model · Nov 17, 2026 · companion to the locked architecture doc (revision 2)

**Scope.** Flow and the semantic model are done. The architecture, the editor and WASM plan and the insight layer each have their own document (links below). This document holds the rest: the generics and inference solver, missing options and language areas, build mode, declaration emit, robustness, open-source readiness, the conformance dashboard and diagnostic ownership. It ends with one master checklist.

**Status markers.** *Verified* means read in the source (`a05b7c7c`, branch `newtypes`). *Search* means found by keyword search only, which is coarse: a feature under another name would show zero hits. *Open* means unverified. Nothing here has been built or run.

## 1. Related documents

| Document | Covers |
| --- | --- |
| [Architecture, revision 2](https://claude.ai/artifact/2VwFMgYojgRg9Gu7sKrnMB) | Layers, queries, engines, flow, semantic model, registries, migration tracks |
| [Editor and WASM expansion](https://claude.ai/artifact/4AqWa2eaUkqmzmLKvvhrnd) | `tsox-ide`, LSP hardening, VS Code extension, WASM project API |
| [Insight layer design](https://claude.ai/artifact/6YL2SckKqtFjodNjgCaWPy) | Opt-in CLI for change impact and agent queries |
| [Insight pitch](https://claude.ai/artifact/TEW47todWEBqJ8byX8AYJn) | Why and for whom |

## 2. Workstreams at a glance

| ID | Workstream | Why | Size | Priority |
| --- | --- | --- | --- | --- |
| W1 | Generics and inference solver | Largest source of call and generic mismatches; a 3.4k-line file is hard to enter | Large | High |
| W2 | Missing options and unused-code diagnostics | Common in real projects, cheap to add | Small to medium | High |
| W3 | Thin language areas (decorators, JSX, `using`, `unique symbol`, module augmentation, triple-slash) | Real code hits them | Medium each | Medium |
| W4 | JavaScript and JSDoc checking | Whole class of projects | Very large | Later |
| W5 | Build mode and project references | Monorepos and CI | Large | Medium |
| W6 | Declaration emit | Library authors need `.d.ts` | Large | Medium |
| W7 | Robustness | Runs in editors, WASM and agents on hostile input | Medium | High before publishing |
| W8 | Open-source readiness | Contributors cannot join without it | Medium | High before publishing |
| W9 | Conformance dashboard and observability | Shows contributors where to help | Small | Medium |
| W10 | Diagnostic ownership | Owner map must use tsox's own code namespace | Small | High, early |

## 3. W1: generics and inference solver

### 3.1 State today

- **Session (verified).** `GenericInferenceSession` in `semantic/solver.rs` (840 lines) collects candidates and resolves them in `finish`. Candidates carry a priority (Low, Normal, Contextual), a variance, a literal-preservation flag, an optional constraint and a top-level flag. Explicit type arguments are locked before inference starts. Covariant candidates win over contravariant ones; contra-only evidence is combined by intersection. `finish` makes several passes so one parameter's answer can feed another.
- **Facade (verified).** The module header calls itself a behavior-preserving adapter that gives a later solver migration one narrow seam.
- **`generics.rs` (verified, 3,426 lines)** does three jobs: (1) inference, including conditional-type `infer`; (2) instantiation, in `substitute_impl`, with a depth cap of 100 that records when it was hit; (3) type-level reduction: conditional, mapped, `keyof`, indexed access, template literals, string mappings, `Record` and variadic tuples.
- **Call site (verified).** `calls.rs` uses the solver in 43 places. It does tsc-style two passes: arguments that are not context-sensitive first, then callbacks typed against the preliminary bindings. It scores overload candidates and reports no-match errors.
- **Gaps (search).** No code for inferring through generic function arguments (the `pipe` and `compose` pattern). tsc uses about a dozen kinds of inference priority; tsox has three.

### 3.2 Target structure

Split by job, a move with no behavior change (the same discipline as architecture step A2):

| Module | Owns |
| --- | --- |
| `semantic/infer/` | session, candidates, priorities, `finish`, constraint and default application |
| `semantic/instantiate/` | substitution, the shared budget (depth, size, steps) |
| `semantic/reduce/` | one file per type operator: conditional, mapped, `keyof`, indexed access, template, string mapping, record, variadic |

The call orchestration stays in the checker (`calls/` after the split): resolving a call, ordering arguments, overload candidates, error reporting.

### 3.3 Contract

The solver answers with the same outcomes as the other engines in the architecture doc:

- `Resolved { bindings, notes }`, `Partial(reason)` (for example budget reached), `Failed(CandidateFailure)`.
- Each reducer returns `Reduced(type)`, `Deferred` (still depends on a type parameter) or `TooComplex`. None is ever turned into a plain `any` without a counter.
- One budget object (depth, node count, steps) replaces scattered constants, is passed explicitly, and its exhaustion is a named reason.

### 3.4 Priorities

Replace the three levels with an explicit priority set that mirrors tsc's (naked type variable, homomorphic mapped type, return type, mapped-type constraint, `keyof` literal, no constraints and so on). Map today's three levels onto it first so nothing changes, then add kinds one at a time, each with a fixture. The exact list of tsc kinds should be copied from the pinned TypeScript 6.0.3 source when this step starts.

### 3.5 Return-type and higher-order inference

1. **Return-type inference.** Treat the contextual return type of a call as a candidate source with its own priority, so `const x: Box<string> = make()` infers `T` from the expected type.
2. **Generic function arguments, in stages.** (a) A single-signature generic argument instantiated in the context of the parameter type. (b) Propagating through `pipe`- and `compose`-style chains. (c) Overloaded generic arguments. Each stage ships behind its own fixtures and is compared with tsc.

### 3.6 Inference trace

An opt-in trace with zero cost when off: each candidate added (parameter, type, priority, variance, source argument), each widening decision, each constraint applied, and the reason the final binding was chosen. It serves contributors debugging a mismatch and later feeds `explain` in the insight layer.

### 3.7 Tests and gates

- A fixture per priority kind and per reducer, with expected output from tsc 6.0.3.
- Identical diagnostics on the corpus after each structural step.
- Targeted code families: argument and overload errors, and assignment errors that come from inferred types.
- Hover-type fixtures for inferred bindings, since hover shows inference results directly.

### 3.8 Order

1. Split into `infer/`, `instantiate/`, `reduce/`.
2. Shared budget and the contract.
3. Reducer registry.
4. Inference trace.
5. Priority set.
6. Return-type inference.
7. Higher-order inference stages.

The probes and `CandidateFailure` from the architecture doc (steps B5 and B6) are the checker-side half of this work and come first or alongside.

## 4. W2: missing options and unused-code diagnostics

### 4.1 What is modelled

`CheckOptions` (verified) holds `strict` and its family (`noImplicitAny`, `noImplicitThis`, `strictNullChecks`, `strictFunctionTypes`, `strictBindCallApply`, `strictPropertyInitialization`, `alwaysStrict`, `strictBuiltinIteratorReturn`), `noUncheckedIndexedAccess`, `allowUnreachableCode`, `useUnknownInCatchVariables`, `exactOptionalPropertyTypes`, `noPropertyAccessFromIndexSignature`, `noImplicitOverride`, `noImplicitReturns`, `noFallthroughCasesInSwitch`, and two module-detection flags.

### 4.2 What has no hits (search)

| Option | What it drives |
| --- | --- |
| `noUnusedLocals`, `noUnusedParameters` | The unused-code diagnostics (the TS6133 family), very common in real projects |
| `isolatedModules` | Per-file restrictions (re-exporting types, `const enum` use, files that are not modules) |
| `verbatimModuleSyntax` | Import and export elision rules and the errors around them |
| `erasableSyntaxOnly` | Errors for syntax that cannot simply be erased (enums, namespaces with values, parameter properties) |
| `allowSyntheticDefaultImports`, `esModuleInterop` (1 file) | Default imports from `export =` modules |
| `resolveJsonModule` | Importing `.json` files |
| `allowUnusedLabels` | Unused label errors |
| `experimentalDecorators`, `jsx` factory and import-source options | Decorator and JSX behavior (see W3) |

### 4.3 Design

- **Options through one accessor.** Every option is read through a method that records a read of that option (the typed read log in the architecture doc), so a unit that never read an option does not recheck when it changes. Do this for the existing options too.
- **Unused analysis as a registered pass.** A `PASSES` entry that runs after typeck. Oxc's semantic model already records every reference to a symbol with read, write and type flags (not verified here), which is the input needed. Rules to model: a write-only variable counts as unused; names starting with `_` are exempt for parameters; destructured elements, imports (per specifier and all-unused), type parameters, private class members and local type declarations each have their own wording and code. Exported symbols are never unused.
- **Contract with `isolatedModules` and `verbatimModuleSyntax`.** These are syntax-level checks that need the import use table (`module_info` in the semantic model), so they come after that model exists.

### 4.4 Gates

A fixture per option and per unused-code case compared with tsc, and a corpus run grouped by the new codes.

## 5. W3: thin language areas (search)

| Area | State | Next step |
| --- | --- | --- |
| Decorators | 1 file mentions them | Decide standard decorators first; `experimentalDecorators` behind its option |
| JSX | 1 checker file; no factory or import-source options | Namespace and factory resolution, intrinsic elements, props checking |
| `using` and `await using` | 2 files | Disposable checks and the related errors |
| `unique symbol` | none | Type form, assignability, declaration rules |
| Module augmentation | none (`declare global` appears in 8 files) | `declare module "x"` merging into an existing module |
| Triple-slash directives | 1 file | `reference path`, `reference types`, `reference lib` handling |
| Generators and async iteration | 9 and 2 files | Review against the fixtures once more is needed |

Each area follows the same rule: add a fixture set first, then the check, then a registry entry with an owner.

## 6. W4: JavaScript and JSDoc checking

No hits for `allowJs`, `checkJs` or JSDoc (search). This is the largest item and changes many assumptions, so it waits. When it starts:

1. Parse JS files and check them with the options `allowJs` and `checkJs`.
2. JSDoc types: `@type`, `@param`, `@returns`, `@typedef`, `@template`, `@callback`, `@import`.
3. CommonJS patterns: `module.exports`, `require`, expando properties, prototype assignment.
4. A separate fixture corpus for JS, since TypeScript's JS-specific rules differ.

Decision to make before starting: whether JS support is a goal for the first public release or a documented later item.

## 7. W5: build mode and project references

Module resolution already handles `references`, `extends`, `paths` and package `exports` (search, with `references` in 4 files; the depth is not verified). There is no build mode and no persistent build info (no `tsbuildinfo` hits).

- **Command.** `tsox build` over a tree of projects, ordered by the reference graph.
- **Up-to-date check by content hash**, not timestamps, using the existing hash-keyed disk cache, so the result is the same on every machine.
- **Outputs.** Each project's declaration output (needs W6) and its own build-info file in tsox's own format. Compatibility with tsc's `tsbuildinfo` is a non-goal.
- **Order and parallelism.** Reuse the scheduler: projects with no dependency between them check in parallel.
- **Gates.** A rebuild after a change rechecks only the dependent projects; the result equals a cold build; the output is deterministic.

## 8. W6: declaration emit

The roadmap in the architecture doc covers JavaScript emit only.

- **Fast path first.** Support the `isolatedDeclarations` style: when exports carry explicit annotations, `.d.ts` can be produced from syntax without checking, in parallel, and cheaply. This also makes build mode (W5) fast.
- **Inferred path second.** For exports without annotations, print the inferred type from the semantic model with the existing type printer (`type_display.rs`, 871 lines), with the rules for what is allowed to appear in a declaration (no private names, no unnamed module types).
- **Gate.** Compare emitted `.d.ts` with tsc's on a sample set; both must also type-check when consumed.

## 9. W7: robustness

The checker runs on untrusted text in editors, in a browser and inside agents, so it must never crash, hang or use unbounded memory.

- **Fuzzing.** Targets for the parser and repair loop, the checker, module resolution and the WASM API; seed with the corpus; run nightly in CI; keep every crasher as a regression file. I did not check whether fuzz targets exist.
- **Panic safety.** A panic in one request or one file becomes a diagnostic or an error for that unit and the process continues. The LSP and the WASM API test this.
- **Limits.** The depth guard and size limits exist (verified: instantiation depth 100 with a recorded flag). Extend the same budget idea to every engine and make exhaustion a named partial result, never silence.
- **Time and memory caps** per unit, with the cancellation token, and a test that a pathological input finishes inside the 5000 ms budget or reports a partial result.
- **`unsafe` audit.** List every `unsafe` block, justify or remove it (the count is not verified).
- **Determinism under fuzz.** The same input gives the same output with threads on and off.

## 10. W8: open-source readiness

| Area | Items |
| --- | --- |
| Legal | Choose the license; add a notice for the vendored TypeScript library files (TypeScript is Apache-2.0, so attribution is required); check the licenses of the corpus repositories before any of it is published |
| Community | `CONTRIBUTING`, code of conduct, `SECURITY` policy, issue and PR templates, `CODEOWNERS` per crate, a decision log |
| First tasks | Label issues by registry entry and fixture: a new narrowing rule, a missing option, a TS code with a fixture. The ownership map (W10) generates the list |
| Releases | crates.io publishing (check name availability), platform binaries from CI, the npm package for WASM, the VS Code extension; one version number across them |
| Stability | A written semver policy for the facade crate (the root crate is the public surface), a minimum Rust version, a changelog, experimental labels on new commands |
| Docs | A short architecture overview for newcomers, a per-crate README, a "how to add a rule" page, the checklist in this document |
| Tooling | `check-layers`, formatting and lint in CI, a scaffold command for a new rule or pass |

## 11. W9: conformance dashboard and observability

- A page generated from each corpus run: pass rate overall and by code family, trend per commit, the files that regressed.
- The fallback counters from the architecture plan (`Unknown(reason)`, `Incomplete`, `TooComplex`) shown beside conformance, so contributors see both what is wrong and what is not yet understood.
- Per-file time and peak memory against the 5000 ms budget, tracked in CI.
- A `--stats` output (the CLI already prints units checked and reused in watch mode) extended with counters by reason.

## 12. W10: diagnostic ownership

tsox does not use tsc's code numbers as its own (verified in `diagnostic_codes.rs`): it has its own `TSR####` variants, each with a map to a tsc code, several variants may share one tsc code, and a 9000s range marks conditions that are not checked yet. Consequences:

- The owner map in the architecture doc (code to module to fixture) must key on `DiagnosticCode` variants, and show the tsc code beside each.
- The 9000s markers are a ready-made backlog of unchecked conditions. List them, with the tsc code each will map to, and tag the easiest as first tasks.
- A test asserts that every variant has an owner, and the compare scripts continue to use the tsc map.

## 13. Order

| Phase | Work | Why here |
| --- | --- | --- |
| 1 | W10 owner map; W2 options accessor and read recording | Small, unblocks contributors and incremental checking |
| 2 | W1 steps 1 to 4 (split, budget, registry, trace); W2 unused-code pass and missing options | Structure first, then common diagnostics |
| 3 | W7 fuzzing and panic safety; W8 legal and community files | Required before publishing |
| 4 | W1 steps 5 to 7 (priorities, return-type, higher-order); W9 dashboard | Behavior changes with a tracked baseline |
| 5 | W3 areas one at a time; W6 declaration emit (fast path) | Language coverage |
| 6 | W5 build mode; W6 inferred path | Needs W6 outputs |
| 7 | W4 JS and JSDoc | Large and optional for first release |

## 14. Master checklist

### W1: generics and inference solver

- [ ] Baseline: corpus diagnostics and solver-related fixtures recorded before any move.
- [ ] `generics.rs` split into `infer/`, `instantiate/`, `reduce/` with identical corpus output.
- [ ] One shared budget object (depth, size, steps) with named exhaustion.
- [ ] Solver contract: `Resolved`, `Partial(reason)`, `Failed(CandidateFailure)`; reducers return `Reduced`, `Deferred` or `TooComplex`.
- [ ] Reducer registry, one file per type operator, each with a fixture.
- [ ] Opt-in inference trace, zero cost when off.
- [ ] Priority set matching tsc 6.0.3, mapped from today's three levels without change first.
- [ ] Return-type inference with its own priority.
- [ ] Higher-order inference in stages (single signature, chains, overloads), each compared with tsc.
- [ ] Probes and `CandidateFailure` in call resolution (architecture steps B5 and B6).

### W2: options and unused code

- [ ] Every option read through an accessor that records the read.
- [ ] `noUnusedLocals`, `noUnusedParameters` and the unused-code pass with fixtures for each case.
- [ ] `isolatedModules`, `verbatimModuleSyntax`, `erasableSyntaxOnly`.
- [ ] `allowSyntheticDefaultImports` and `esModuleInterop` rules, `resolveJsonModule`, `allowUnusedLabels`.
- [ ] Each option documented with the codes it controls.

### W3: language areas

- [ ] Decorators: standard first, experimental behind its option.
- [ ] JSX: namespace, factory and import-source options, props checking.
- [ ] `using` and `await using` checks.
- [ ] `unique symbol`.
- [ ] Module augmentation.
- [ ] Triple-slash directives.
- [ ] Generators and async iteration reviewed against fixtures.

### W4: JavaScript and JSDoc

- [ ] Decision: in the first release or later.
- [ ] If in scope: parse and check JS, JSDoc forms, CommonJS patterns, a JS fixture corpus.

### W5: build mode

- [ ] `tsox build` over the reference graph.
- [ ] Up-to-date check by content hash.
- [ ] Own build-info format, deterministic.
- [ ] Parallel projects through the scheduler.
- [ ] Rebuild equals cold build; only dependents recheck.

### W6: declaration emit

- [ ] Fast path from explicit annotations without checking.
- [ ] Inferred path through the semantic model and the type printer.
- [ ] `.d.ts` compared with tsc on a sample and consumed by a type-check.

### W7: robustness

- [ ] Fuzz targets for parser and repair, checker, resolver and WASM API; nightly CI run.
- [ ] Crasher regression corpus.
- [ ] A panic in a request or unit becomes an error and the process survives.
- [ ] Every engine reports exhaustion as a named partial result.
- [ ] Per-unit time and memory caps tested on pathological input.
- [ ] `unsafe` blocks listed and justified or removed.
- [ ] Determinism holds with threads on and off.

### W8: open-source readiness

- [ ] License chosen; notice for vendored TypeScript library files; corpus licenses checked.
- [ ] `CONTRIBUTING`, code of conduct, `SECURITY`, templates, `CODEOWNERS`, decision log.
- [ ] First-task issues generated from the owner map and the 9000s markers.
- [ ] crates.io, platform binaries, npm and extension release pipeline; one version number.
- [ ] Semver policy for the facade crate, minimum Rust version, changelog.
- [ ] Newcomer overview, per-crate READMEs and a "how to add a rule" page.

### W9: dashboard and observability

- [ ] Generated conformance page by code family with a trend per commit.
- [ ] Fallback counters shown beside conformance.
- [ ] Per-file time and memory tracked in CI.
- [ ] `--stats` extended with counters by reason.

### W10: diagnostic ownership

- [ ] Owner map keyed on `DiagnosticCode` variants, with the tsc code shown.
- [ ] Test that every variant has an owner.
- [ ] 9000s markers listed as a backlog with their target tsc codes.

## 15. Risks and open decisions

- **Keyword evidence.** Every "no hits" above is a search result. Before starting a workstream, read the code once to confirm the gap is real.
- **W1 is the highest-risk item.** Changing priorities changes inference results across the corpus. The baseline, one change at a time, and the trace are the controls.
- **Scope.** Ten workstreams cannot run at once. The order table is the recommendation; W4 and the inferred half of W6 are the first to cut.
- **Corpus licensing** may constrain what can be published with the repository.
- **Decisions:** JS support in the first release or not; license; whether declaration emit ships before build mode; the standard-versus-experimental decorator default.
