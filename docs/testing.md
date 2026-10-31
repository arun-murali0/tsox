# Testing and Performance

The test suite is organized by semantic capability. A test name should tell a contributor what behavior is protected without requiring knowledge of an internal development version.

## Test organization

Each semantic milestone has:

1. a Rust integration test in `tests/`;
2. focused TypeScript fixtures in `tests/fixtures/`;
3. a matching document in `docs/`.

For example:

```text
Generics (one doc, several test suites -- see docs/architecture/generics.md for why)
├── tests/generics_tier1.rs, generics_tier2.rs, generic_classes.rs, recursive_types.rs, builtin_generics.rs
├── tests/fixtures/generics-tier1/, generics-tier2/, generic-classes/, recursive-types/, builtin-generics/
└── docs/architecture/generics.md
```

The Rust test defines the executable contract. The fixture demonstrates the TypeScript behavior. The documentation explains the semantic and architectural reason.

## Naming

Use semantic capability names:

```text
foundation
structural_types
literal_widening
control_flow_narrowing
classes_inheritance
expression_program_checking
parameters_destructuring
generics_tier1
```

Do not introduce numeric release-style labels for semantic test suites. Cargo package releases may still use normal semantic versioning; that is separate from development milestones.

## What a good fixture tests

A fixture should make one semantic claim whenever practical.

Prefer:

```text
generic_constraint_rejects_invalid_argument.ts
```

over a large fixture that combines unrelated generic, class, and control-flow behavior.

When a bug spans multiple semantic systems, a focused regression should still identify the interaction in its filename or test name.

## Required validation

Before merging a semantic change:

```bash
cargo fmt --all -- --check
cargo check --all-targets --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

The repository shortcut is:

```bash
./scripts/ci.sh
```

## Performance validation

The repository contains:

- Criterion benchmarks in `benches/checker_benchmark.rs`;
- module graph benchmarks in `benches/module_graph_benchmark.rs` (graph build, layers, cycles, change detection and the parallel project check), which only build with `--features module-resolution`;
- cache and memo counters, readable from a `CheckSession` after a check (`last_metrics`), for regressions that wall-clock time hides;
- foundation benchmarks in `benches/foundation_benchmark.rs` (the scratchpad against a `Vec`, and the incremental layer's unchanged save and valid read), which need `module-resolution`, `incremental` and `scratchpad`;
- IAI callgrind benchmarks in `benches/checker_iai.rs`;
- shared complex fixtures in `benches/support/complex_fixtures.rs`;
- heap profiling in `examples/dhat_heap.rs`.

The shared benchmark generators intentionally exercise:

- unions and discriminant narrowing;
- nested object resolution;
- class inheritance;
- generic inference/substitution;
- destructuring;
- mixed realistic workloads.

A semantic change that passes tests but creates a significant benchmark regression should be investigated before merging.

## Compatibility harness

tsox has three tools that compare its diagnostics against real TypeScript
implementations, and they exist separately because they answer different
questions rather than being three ways to do the same thing:

```text
scripts/compare-local.sh   -> "did this change disagree with tsc on our own fixtures?"
bin/compare-tsc.rs         -> "does tsox agree with tsc on a small, curated set?"
scripts/harness.sh         -> "how does tsox perform/compare on a real project?"
```

**`scripts/compare-local.sh`** is the fast, local, everyday one. It runs the
entire `tests/fixtures/` suite (every milestone, not a subset) against `tsc
--strict` once, informationally rather than pass/fail — many fixtures under
`tests/fixtures/` deliberately exercise tsox's own recovery behavior (one
unresolvable class member making the whole class unsupported, say) and are
*expected* to diverge from `tsc`, so this never blocks on that. It runs `tsc`
once per file rather than batched, specifically because none of these fixtures
use imports/exports, so batching them into one `tsc` invocation would make
`tsc` treat every fixture as part of the same program and produce spurious
duplicate-identifier errors from names reused across unrelated fixtures
(`Point`, `Animal`, `Counter`...). Output is grouped by milestone and each file
is marked `MATCH`, `GAP` (only `tsc` reports an error — a check not built yet),
`FALSE POSITIVE` (only tsox reports one — rejecting code `tsc` accepts, the
more serious kind), or `MIXED`.

```bash
./scripts/compare-local.sh                        # all of tests/fixtures
./scripts/compare-local.sh tests/fixtures/generics-tier1
./scripts/compare-local.sh path/to/one_file.ts
```

**`bin/compare-tsc.rs`** is the in-process, errors-only sibling. Instead of
shelling out to a `tsox` binary, it calls `TypeChecker::check_source`
directly and only shells out to `tsc` for the other half of the comparison.
It's scoped to `tests/tsc-conformance/`, a small, deliberately curated
directory kept separate from `tests/fixtures/` for the same reason
`compare-local.sh` treats that directory as informational: mixing in fixtures
that are supposed to diverge from `tsc` would make a pass/fail conformance
check meaningless.

**`scripts/harness.sh`** is the one aimed at a real external project (Zustand
by default) rather than curated fixtures — see the "Running it locally" section
in the README for usage. It benchmarks `tsc`, `tsgo`, and `tsox` against the
same project and reports speed, peak memory and CPU, and how tsox's errors
compare with tsc's and tsgo's (matches, false positives, gaps, lines that
agree, files by class), plus what tsox skipped. It saves every run's raw
output and timings with the compiler versions, project revision and exact
commands, so each figure can be recomputed by hand. `--dhat` adds the
allocation profile from `examples/dhat_heap`; the criterion benches
(`cargo bench`) stay the place for fine-grained speed.

None of the three are a semantic-equivalence proof. A matching diagnostic or
exit code does not establish that two implementations have identical type
semantics — they're compatibility investigation tools, and the strength of
that signal is different for each: `compare-tsc` is the strictest (curated,
pass/fail), `compare-local` is the broadest (whole suite, informational),
`harness.sh` is the most realistic (a real project) but also the least
controlled (real projects use language features tsox may not model yet at
all, which shows up as noise rather than signal until module resolution and
broader feature coverage land).

## Adding a new semantic capability

Use this sequence:

```text
1. Define the semantic rule.
2. Add a focused fixture.
3. Add the integration assertion.
4. Implement the smallest reusable semantic primitive.
5. Keep Oxc-specific traversal in bridge/.
6. Run the complete regression suite.
7. Run representative benchmarks.
8. Update the matching documentation.
```

This keeps tests, implementation, and documentation synchronized.
