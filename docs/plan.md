---
title: Work plan
---

# Work plan: W1 to W10

The remaining work is split into ten workstreams. The design of each one, with its gates and a
checklist, is in the [gap register](architecture/remaining-work-gap-register.md). The state of each
one is taken from the [progress notes](architecture/new-architecture-progress.md). This page is
the short version of both.

<Workstreams />

## The recommended order

Not everything can run at once. The order puts structure before behaviour: small enabling work
first, the solver split before the solver changes, and the work that must precede publishing
before the work that does not.

<WorkOrder />

Two cuts are named in advance. W4 (JavaScript and JSDoc) and the inferred half of W6
(declaration emit) go first if the scope has to shrink.

## W1 in detail: generics and inference solver

W1 is the highest-risk workstream: changing inference priorities changes results across the
corpus. The controls are a recorded baseline before any move, one change at a time, and the
inference trace.

| Step | What it does | State |
| --- | --- | --- |
| 1. Split | `generics.rs` becomes `infer/`, `instantiate/` and `reduce/`, with identical tests. | <span class="badge done">Done</span> |
| 2. Shared budget | Every cut-off is named and counted per reason (`CutReason`). One `Budget` object passed explicitly is still open. | <span class="badge partial">Partly</span> |
| 3. Reducer contract | Six operators return `Reduced`, `Deferred` or `TooComplex`. `Record` is still direct, and the solver's own `Resolved`, `Partial`, `Failed` outcome is open. | <span class="badge partial">Partly</span> |
| 4. Inference trace | `GenericInferenceSession::with_trace`, with zero cost when off. Not yet behind a CLI flag. | <span class="badge done">Done</span> |
| 5. Priority set | Replace three priority levels with tsc's set, mapped one to one first. Needs the priority list from the pinned TypeScript 6.0.3 source. | <span class="badge open">Open</span> |
| 6. Return-type inference | The contextual return type of a call becomes a candidate source, so `const x: Box<string> = make()` infers `T`. | <span class="badge open">Open</span> |
| 7. Higher-order inference | Generic function arguments in three stages: one signature, `pipe` and `compose` chains, overloaded arguments. | <span class="badge open">Open</span> |

**Gate for every step.** A fixture per priority kind and per reducer with output from tsc 6.0.3,
and identical diagnostics on the corpus after each structural step.

## W2 in detail: options and unused-code diagnostics

| Step | What it does | State |
| --- | --- | --- |
| Options accessor and read log | Every option is read through `CheckContext::option`, which records the read. An option no unit reads re-checks none of the 13 units in the measurement (it was 13 of 13). | <span class="badge done">Done</span> |
| Unused-code pass | `noUnusedLocals` and `noUnusedParameters` as a registered pass that runs after type checking. | <span class="badge open">Open</span> |
| Missing options | `isolatedModules`, `verbatimModuleSyntax`, `erasableSyntaxOnly`, `allowSyntheticDefaultImports` and `esModuleInterop`, `resolveJsonModule`, `allowUnusedLabels`. | <span class="badge open">Open</span> |
| Documentation | Each option documented with the codes it controls. | <span class="badge open">Open</span> |

The read log also produced a finding worth knowing: 8 of the 19 `CheckOptions` flags are never
read by the checker, so it behaves as if the strict family were always on. The full list is in
the [progress notes](architecture/new-architecture-progress.md).

**Gate.** A fixture per option and per unused-code case compared with tsc, and a corpus run
grouped by the new codes.

## Risks and open decisions

- **Keyword evidence.** Every "no hits" in the register is a search result. Read the code once
  before starting a workstream to confirm the gap is real.
- **Scope.** Ten workstreams cannot run at once.
- **Corpus licensing** may limit what can be published with the repository.
- **Decisions still to make:** whether JavaScript support is in the first release, the licence,
  whether declaration emit ships before build mode, and the standard-versus-experimental
  decorator default.
