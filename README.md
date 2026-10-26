# Tsoxide (tsox)

> A Rust-based experimental TypeScript type checker built on top of Oxc.

## ⚠️ Experimental, not production-ready

**Tsoxide (tsox) is experimental software. Do not use it as a production TypeScript compiler**

The project is actively evolving. TypeScript compatibility is incomplete (there are no tuples, `keyof`, mapped or conditional types, and classes, enums and overloads do not yet cross files), and the compatibility harness is an experimental comparison tool rather than a correctness proof.

## What is Tsoxide?

Tsoxide (tsox) explores how much of the TypeScript type system can be implemented in Rust while reusing the mature front-end infrastructure provided by [Oxc](https://oxc.rs/).

We deliberately do **not** try to rebuild everything.

Oxc handles the expensive front-end pieces:

- TypeScript parsing
- AST representation
- semantic symbols/scopes/references
- source information
- module resolution where adopted

tsox owns the part that is the reason this project exists:

- semantic type representation
- `TypeId` and arena-backed type storage
- structural subtyping and assignability
- declaration/type resolution
- expression type inference
- TypeScript-specific control-flow narrowing
- parameter and binding semantics
- diagnostics
- generic inference and substitution, with advanced type-level semantics planned
- intersection types
- stable keys and portable declarations, so a type written in one file is used in another

The design goal is a clean boundary:

```text
                    TypeScript source
                           │
                           ▼
                    ┌─────────────┐
                    │     Oxc     │
                    │ parser / AST│
                    │  semantic   │
                    └──────┬──────┘
                           │
                           ▼
                    ┌─────────────┐
                    │  tsox       │
                    │   bridge    │
                    └──────┬──────┘
                           │
                           ▼
              ┌────────────────────────┐
              │    Semantic Type Core  │
              │ Type / TypeId / Arena  │
              │ namespace / subtyping │
              └───────────┬────────────┘
                          │
                    semantic queries
                          │
             ┌────────────┼────────────┐
             ▼            ▼            ▼
        inference     narrowing    diagnostics
             │            │
             └────────────┘
                    │
                    ▼
              checker result
```

## Why build it?

The project exists as an engineering and research exercise around a difficult question:

> Can a fast Rust implementation build a useful, understandable TypeScript semantic checker while consuming mature compiler front-end infrastructure instead of recreating it?

The project is also an architecture study. The implementation is intentionally built in small, testable milestones so each semantic decision can be inspected, measured, and changed without turning the checker into a collection of AST-specific special cases.

## Current implementation

The project records progress using **semantic milestone names**, not opaque version labels. The current baseline is **Cross-File Types**: a type written in one file is used in another, imports are diagnosed, and import cycles settle (see [docs/architecture/cross-file-types.md](docs/architecture/cross-file-types.md)), on top of generics that are substantially complete across functions, interfaces, type aliases and classes (see [docs/architecture/generics.md](docs/architecture/generics.md)). `checklist.md` at the repository root tracks the full TypeScript type-system surface item by item, and `BUILD.md` tracks the current build order beyond it.

```text
Foundation
Structural Types
Literal Widening
Control-Flow Narrowing
Classes and Inheritance
Expression and Program Checking
Parameters and Destructuring
Generics
Narrowing completion, sessions and file identity
Module Resolution
Architecture Foundation
Declaration Slots and Named Nodes
Intersection Types
Cross-File Types                 <- current baseline
```

These names are deliberately descriptive so a contributor can infer the purpose of a test suite or document without first learning an arbitrary version history.

## Repository structure

```text
src/
├── arena.rs
├── types.rs
├── subtyping.rs
├── namespace.rs
├── symbol_map.rs
├── type_annotation.rs
├── type_display.rs
├── explain.rs             # the chain of reasons under a not-assignable error
├── lib_global_types.rs    # generated list of tsc's global type names
├── keys.rs                # ModuleKey, DeclKey, SigHash, NodeHash
├── stable_hash.rs         # canonical node encoding and Merkle hash
├── stable_walk.rs         # the arena walk that feeds stable_hash
├── portable.rs            # declarations as arena-free data, the importer, ImportSource
├── unit.rs                # a file as a unit of work: UnitContext, UnitOutput, Worker
├── diagnostics.rs
├── diagnostic_codes.rs
├── diagnostic_messages.rs
├── diagnostic_view.rs
├── fxhash.rs
├── line_index.rs
├── project.rs
├── module_resolution/     # opt-in `module-resolution` feature
├── topology.rs            # petgraph view of the module graph (`module-resolution`)
├── incremental.rs         # Salsa input and first query (`incremental`)
├── scratchpad.rs          # bump scratch for one worker (`scratchpad`)
├── error.rs
├── wasm.rs
├── semantic/
│   ├── mod.rs
│   ├── generics.rs
│   └── queries.rs
└── bridge/
    ├── context.rs
    ├── declare.rs
    ├── imports.rs
    ├── exports.rs
    ├── narrow.rs
    ├── parse.rs
    ├── unresolved_names.rs
    ├── unreachable_code.rs
    ├── expressions/
    │   ├── mod.rs
    │   ├── core.rs
    │   ├── assignment.rs
    │   ├── binary.rs
    │   ├── calls.rs
    │   ├── elaborate.rs
    │   ├── excess.rs
    │   ├── functions.rs
    │   ├── logical.rs
    │   ├── members.rs
    │   └── objects.rs
    └── statements/
        ├── mod.rs
        ├── functions.rs
        ├── classes.rs
        ├── control_flow.rs
        ├── variables.rs
        ├── patterns.rs
        └── support.rs

tests/                     # one integration test per milestone or feature (42 files)
├── foundation.rs
├── structural_types.rs
├── literal_widening.rs
├── control_flow_narrowing.rs
├── classes_inheritance.rs
├── expression_program_checking.rs
├── parameters_destructuring.rs
├── generics_tier1.rs
├── intersection_types.rs
├── unit_exports.rs, unit_imports.rs, project_imports.rs, import_diagnostics.rs
├── ...
└── module_resolution.rs   # needs --features module-resolution

tests/fixtures/
├── foundation/
├── structural-types/
├── literal-widening/
├── control-flow-narrowing/
├── classes-inheritance/
├── expression-program-checking/
├── parameters-destructuring/
├── generics-tier1/
├── generics-tier2/
├── generic-classes/
├── recursive-types/
├── builtin-generics/
├── interface-methods/
└── unreachable-code/

tests/module-resolution-fixtures/   # small projects on disk, not checked by the tsc comparison

docs/
├── README.md
├── architecture.md
├── architecture/          # one case study per subsystem: arena, narrowing, generics,
│                          # intersections, module-resolution, cross-file-types,
│                          # foundation-dependencies
├── case-study.md
├── decisions.md
├── roadmap.md
├── testing.md
├── purpose-and-overdesign.md
└── <other semantic milestone documents>

scripts/
├── ci.sh
├── harness.sh
├── compare-local.sh
├── ts-diag-tool/          # tsc/tsgo diagnostic comparison, see README's own section on it
└── enable-hooks.sh

benches/                 # Criterion and IAI benchmarks
examples/                # Small usage and profiling examples
bin/                     # CLI target
checklist.md             # full TypeScript type-system surface, item-by-item status
BUILD.md                 # current build order, derived from checklist.md
```

## Architecture direction

The current implementation deliberately builds on Oxc instead of recreating its parser or front end. tsox owns the TypeScript semantic layer.

The next architectural boundary is the small `semantic::queries` API. It is intentionally read-only and narrow today. As generic inference, indexed access, overloads, and advanced type operations grow, semantic queries can expand without forcing AST-facing feature modules to know how the underlying algorithms are stored.

The future direction is:

```text
Oxc
  ↓
bridge feature modules
  ↓
semantic queries
  ↓
type relations / generics / flow / resolution
  ↓
semantic facts + diagnostics
  ↓
tooling or typed IR
```

This is an incremental boundary, not a second compiler architecture.

## Development

Format:

```bash
cargo fmt --all
```

Check:

```bash
cargo check --all-targets --all-features
```

Lint:

```bash
cargo clippy --all-targets --all-features -- -D warnings
```

Test:

```bash
cargo test --all-targets --all-features
```

Run the repository quality gate:

```bash
./scripts/ci.sh
```

## Semantic milestones and regression contracts

Each semantic milestone maps directly to an executable regression suite. These are development contracts, not package release versions.

Start with the [documentation index](docs/README.md), then inspect the relevant fixture directory and integration test together.

| Milestone | Test runner | Fixture directory | Documentation |
| --- | --- | --- | --- |
| Foundation | `tests/foundation.rs` | `tests/fixtures/foundation/` | [Foundation](docs/foundation.md) |
| Structural Types | `tests/structural_types.rs` | `tests/fixtures/structural-types/` | [Structural Types](docs/structural-types.md) |
| Literal Widening | `tests/literal_widening.rs` | `tests/fixtures/literal-widening/` | [Literal Widening](docs/literal-widening.md) |
| Control-Flow Narrowing | `tests/control_flow_narrowing.rs` | `tests/fixtures/control-flow-narrowing/` | [Control-Flow Narrowing](docs/architecture/narrowing.md) |
| Classes and Inheritance | `tests/classes_inheritance.rs` | `tests/fixtures/classes-inheritance/` | [Classes and Inheritance](docs/classes-inheritance.md) |
| Expression and Program Checking | `tests/expression_program_checking.rs` | `tests/fixtures/expression-program-checking/` | [Expression and Program Checking](docs/expression-program-checking.md) |
| Parameters and Destructuring | `tests/parameters_destructuring.rs` | `tests/fixtures/parameters-destructuring/` | [Parameters and Destructuring](docs/parameters-destructuring.md) |
| Module Resolution | `tests/module_resolution.rs` (feature `module-resolution`) | `tests/module-resolution-fixtures/` | [Module Resolution](docs/architecture/module-resolution.md) |
| Intersection Types | `tests/intersection_types.rs`, `tests/intersection_overloads.rs` | `tests/fixtures/intersection-types/`, `intersection-calls/`, `intersection-edges/` | [Intersection Types](docs/architecture/intersections.md) |
| Cross-File Types | `tests/unit_exports.rs`, `tests/unit_imports.rs`, `tests/project_imports.rs`, `tests/import_diagnostics.rs` | sources inline in the tests | [Cross-File Types](docs/architecture/cross-file-types.md) |
| Architecture Foundation | unit tests in `src/topology.rs`, `src/incremental.rs`, `src/scratchpad.rs` | none | [Architecture Foundation](docs/architecture/foundation-dependencies.md) |
| Generics | `tests/generics_tier1.rs`, `tests/generics_tier2.rs`, `tests/generic_classes.rs`, `tests/recursive_types.rs`, `tests/builtin_generics.rs`, `tests/interface_methods.rs` | `tests/fixtures/generics-tier1/`, `generics-tier2/`, `generic-classes/`, `recursive-types/`, `builtin-generics/`, `interface-methods/` | [Generics](docs/architecture/generics.md) |


## Documentation

Start with [docs/README.md](docs/README.md) for the milestone map and architecture.

- [Architecture](docs/architecture.md): ownership, dependencies, semantic boundaries, and future scaling.
- [Roadmap](docs/roadmap.md): the next semantic capabilities and their intended order.
- [Module Resolution](docs/architecture/module-resolution.md): the opt-in project layer (resolver, module graph, cycles, layers, parallel project check).
- [Cross-File Types](docs/architecture/cross-file-types.md): stable keys, portable declarations, units, imports and exports, and where the code differs from the plan.
- [Arena](docs/architecture/arena.md): ids, interning, `Ref` slots, named nodes and the performance phases.
- [Case study](docs/case-study.md): why the checker is built the way it is, one decision per section.
- [Decisions](docs/decisions.md): the choices that closed the open questions in `HLD.md`, with what would reopen each.
- [Architecture Foundation](docs/architecture/foundation-dependencies.md): the optional dependencies from the design documents (petgraph, Salsa, Bumpalo, mimalloc), each behind its own feature and not yet used by the checker.
- [Testing](docs/testing.md): fixture conventions, regression policy, benchmarks, and compatibility harness.
- [Purpose and boundaries](docs/purpose-and-overdesign.md): why the project exists and what it deliberately does not rebuild.

Each milestone document explains the semantic rule, implementation choice, and protected behavior for that capability.

## Compatibility harness

`scripts/harness.sh` compares tsox with `tsc` and `tsgo` on one real project and writes a report you can check by hand. The program is `bin/harness.rs`.

It measures:

- **Speed:** wall time of each compiler after warm-up runs, with the runs interleaved so a slow minute hurts all three alike. Median, mean, standard deviation, min, max, quartiles and a noise flag.
- **Memory and CPU:** peak resident memory and CPU time of each process. Each program is started by a small measuring process, because the kernel otherwise counts the parent's size as the child's.
- **Agreement:** tsox's errors against tsc's and tsgo's, matched by file, line and tsc error code. It counts matches, false positives (tsox errors the reference lacks), gaps (reference errors tsox misses), lines that agree, and files by class. tsc and tsgo are also compared with each other, to show whether the reference itself is consistent.
- **Coverage:** what tsox skipped. Every `TSR9xxx` warning is a construct it did not check, and a partial export is a file whose exports are incomplete. The report says how many agreeing files tsox fully checked, since agreement on a skipped file is weak evidence.

**It is not a semantic-equivalence oracle.** A matching result does not prove two compilers implement the same TypeScript semantics, and tsox checks a smaller language than tsc, so a speed ratio only means something next to the agreement numbers.

### Running it locally

```bash
./scripts/harness.sh
```

This clones Zustand (by default), installs its dependencies with its own lockfile, installs tsc and tsgo under `.harness/tools`, builds `tsox` in release mode, and runs everything. Open `.harness/results/<UTC time>/report.md`.

Each results folder holds `report.md`, `summary.json`, `per-file.csv`, `tsox-false-positives.txt`, `tsox-missed.txt`, `tsc-vs-tsgo-differences.txt`, `REPRODUCE.sh` with the exact commands, and `raw/` with the output and `timings.csv` of every run.

Common variations:

```bash
# Your own project instead of Zustand (needs a tsconfig.json)
./scripts/harness.sh /path/to/your/ts/project

# More timed runs for steadier medians
./scripts/harness.sh --runs 10

# Also run the allocation profile of the benchmark workloads (examples/dhat_heap)
./scripts/harness.sh --dhat

# A pinned Zustand revision, and exit 1 if the run is invalid or tsox has false positives
./scripts/harness.sh --ref <commit> --strict

# Every option
./scripts/harness.sh --help
```

Use a release build of tsox for speed and memory. The report marks a debug build as not meaningful, and marks the whole run invalid when the compilers cannot run or the project's dependencies are missing. Linux and macOS, 64-bit.

### Running it in CI

The harness is **not** part of the `ci.sh` quality gate and never runs automatically on push or pull request — it hits the network, depends on an external fixture repository, and is expected to report diagnostics that disagree with `tsc`/`tsgo` given the current state of the project (see [Category 2/3 of the roadmap](docs/purpose-and-overdesign.md)), so it must not block merges.

It can be run on demand from GitHub Actions instead: go to **Actions → Compatibility Harness → Run workflow**. Results are uploaded as a downloadable artifact (`harness-results`), not committed to the repository.

### Fixture-level three-way report (on demand)

`scripts/harness.sh` above benchmarks against one real project (Zustand, by default). `scripts/ts-diag-tool/compare3.js` applies the same three-way idea (`tsc`, `tsgo`, `tsox`) to `tests/fixtures/` instead — per-fixture diagnostics, timing, and memory, plus a browsable HTML report. Like the harness above, this is on-demand tooling, not part of the fast, CI-gated loop described under "Fixture-level diagnostic comparison" below.

```bash
# tsgo needs its own scratch install -- see "Fixture-level diagnostic
# comparison" below: the plain `typescript` package now resolves to v7+,
# which drops the classic JS compiler API entirely, so it can't share the
# typescript@6.0.3 install pinned in scripts/ts-diag-tool/
mkdir -p /tmp/tsgo-scratch && cd /tmp/tsgo-scratch && npm init -y && npm install typescript@7

cd /path/to/tsox
node scripts/ts-diag-tool/compare3.js tests/fixtures \
  target/release/tsox \
  /tmp/tsgo-scratch/node_modules/.bin/tsc \
  --html report.html
```

Open `report.html` in a browser: one card per compiler (error count, wall-clock time, peak memory), pairwise line-agreement counts, and an expandable per-file breakdown. `--json` gives the same data as structured output instead of the HTML/table view.

Two measurement caveats worth knowing before reading too much into the numbers:
- **tsc's memory figure is this Node process's own peak RSS**, not an isolated subprocess measurement like the other two get — there's no in-process way to isolate just the compiler API's own memory use, so this number includes the comparison tool's own overhead (V8, `cli-table3`, etc.).
- **tsgo runs one subprocess per file** (same reason `check-fixtures.js` gives `tsc` one `ts.Program` per file: fixtures across the suite commonly reuse top-level names, and non-module `.ts` files share a global scope, so batching them would produce spurious duplicate-identifier errors unrelated to real type checking). Its reported time is the *sum* across all those subprocess invocations, and its memory is the *max* peak RSS seen across them — not a single steady-state number the way a one-shot `tsc --project` run would give you.

Memory figures need [GNU time](https://www.gnu.org/software/time/) (`/usr/bin/time -v`) on your `PATH`; without it, `compare3.js` still runs and reports timing, just without the memory columns.

## Fixture-level diagnostic comparison (`scripts/ts-diag-tool/`)

Where the harness above answers "is tsox roughly as fast/compatible as tsc on a real project", this fast, CI-gated tool answers a narrower, more precise question asked continuously: "for each individual fixture under `tests/fixtures/`, which exact diagnostic (code, line, message) does tsox produce versus tsc, and where do they disagree?"

It uses the real TypeScript compiler API (not text-parsing) for `tsc`, so diagnostics are read as structured `ts.Diagnostic` objects — no regex against pretty-printed CLI output, which broke down on type names containing parens or colons in an earlier version of this tooling.

### Setup

```bash
cd scripts/ts-diag-tool
npm install
```

This pins `typescript@6.0.3` deliberately: the plain `typescript` package now resolves to v7+, which ships the native/Go-ported compiler (`tsgo`) instead of the classic JS one, and that package no longer exposes `ts.createProgram`/`ts.Diagnostics` at all.

### Running it locally

```bash
./scripts/compare-local.sh                              # every fixture under tests/fixtures
./scripts/compare-local.sh tests/fixtures/generics-tier1 # one folder
./scripts/compare-local.sh path/to/one_file.ts           # one file
./scripts/compare-local.sh tests/fixtures --only-differ  # hide fixtures where both sides fully agree
./scripts/compare-local.sh tests/fixtures --differ       # list only the lines where tsox and tsc disagree (gap / false positive / mismatch)
./scripts/compare-local.sh tests/fixtures --json         # structured output, e.g. for scripting
```

Prints one table per fixture folder: file, line, tsox's diagnostic, tsc's diagnostic, and a per-line/per-file verdict (`match`, `tsc extra (gap)`, `tsox extra (fp)`). A **gap** (tsc catches something tsox doesn't yet) is expected on a checker still being built out feature by feature. A **false positive** (tsox rejects code tsc accepts) is the more damaging kind, since it breaks valid code rather than under-checking it.

Builds `target/release/tsox` automatically if it isn't already there, and `npm install`s on first run if needed.

### Running it in CI

Unlike the harness above, this one **is** wired into CI: the `tsc-drift` job in `ci.yml` runs `compare-local.sh --json` against the full `tests/fixtures/` suite on every push and pull request, and fails the build only if the false-positive count exceeds what `scripts/ts-diag-tool/baseline.json` currently allows. Gaps are never gated on — they're expected while the checker is still being built out feature by feature; only false positives (which break valid code, rather than just under-checking it) block a merge.

## Security

See [SECURITY.md](SECURITY.md) for vulnerability reporting and security expectations.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

When changing compiler behavior:

1. identify the semantic rule
2. inspect the relevant stage fixtures
3. implement the smallest coherent semantic change
4. add or update a regression fixture
5. run the full quality gate

## License

See [MIT License](LICENSE-MIT) or [Apache License](LICENSE-APACHE).
