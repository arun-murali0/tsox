---
title: How tsox was built
---

# How tsox was built

tsox was built in small, ordered steps. Each step added one capability, came with its own tests
and left a document that tells it as plan, issue, change and gain. This page is the map: the
order the steps were taken in, what each one is guarded by, and where its document is.

The names describe capabilities, not release versions. The numbers only show the order.

## The three rules behind the order

Most choices in the steps below follow from three priorities, in this order. The full argument
is in the [case study](case-study.md).

1. **A wrong error is worse than a missing one.** A checker that reports a false error loses
   trust. When it cannot decide, it keeps the wider type, drops the narrowing, or says nothing.
2. **Agree with `tsc`, measured.** Every fixture runs through real `tsc --strict` and through
   tsox, and CI fails if the number of false positives rises. See [testing](testing.md).
3. **Keep the common path cheap.** Arena ids, interning and small inline stacks keep a large
   file inexpensive, but a design that is fast and wrong loses to rule 1.

## Part 1: One file at a time

The first seven steps build a checker for a single file: primitives, objects, literals,
narrowing, classes, expressions and parameters.

<Journey :part="1" />

## Part 2: Generics and flow

Generic inference and complete narrowing decide how much of everyday TypeScript the checker can
follow, so they came before any project-scale work.

<Journey :part="2" />

## Part 3: From one file to a project

Imports first, then a sturdier core (incremental state, declaration slots, intersections), then
the identity that lets a declaration cross files, and finally the engine that checks a whole
project and re-checks only what a change reaches.

<Journey :part="3" />

## The order inside the project engine

The last step was itself built in an order, and the order was deliberate. Each part waited for
the contract it depends on. The [project engine](architecture/project-engine.md) page has the
reasoning and the exit gate.

1. **Measure partial results first.** Evidence before redesign.
2. **Fix the unit contract before the scheduler.** Whether a cycle is one unit decides what a
   scheduler schedules. See [cycles and settlement](architecture/cycles-and-settlement.md).
3. **Then the scheduler and the pipeline.** Dependency counts, deterministic publication,
   cancellation, one read per file. See [scheduler](architecture/scheduler.md).
4. **Configuration beside it.** It removes errors caused by incomplete project inputs. See
   [project configuration](architecture/project-configuration.md).
5. **Library and ambient declarations.** Names that are not imported, independent of file order.
   See [library and ambient declarations](architecture/library-and-ambient-declarations.md).
6. **The query layer, last of the structure.** Reuse needs stable units and stable publication,
   or a cache serves stale answers. See [query layer](architecture/query-layer.md).
7. **Safety throughout.** A recursion guard added late would have to be added to every walk at
   once. See [safety net and limits](architecture/safety-net-and-limits.md).

## What every step leaves behind

- **A test suite named after the milestone**, so a contributor can tell what it covers from the
  file name alone.
- **A document** in the voice of a case study: what was planned, the issue found, what was
  changed and what it gained.
- **A row in the milestone table** of the [documentation index](README.md).

Later work extends these steps instead of adding a new one. The suites for advanced narrowing,
generic bounds, sessions and unreachable code are listed under the milestones table in the
[documentation index](README.md).

## Where the work goes next

The steps above are done. What is open is tracked in two places:

- the [work plan](plan.md), which breaks the remaining work into ten workstreams, W1 to W10, with
  the status of each;
- the [roadmap](roadmap.md), which lists the next items in the order the evidence suggests.
