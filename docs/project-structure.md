# Project structure

Cargo workspace. Each crate depends only on the ones above it in this list (enforced by
`cargo xtask layering`); the dependency rule and the reasons are in
[docs/architecture/workspace.md](architecture/workspace.md).

```text
crates/
├── tsox-base/             # no tsox dependencies
│   └── src/               # keys, stable_hash, diagnostics (+ codes, view), file_id, fxhash,
│                          # line_index, project, check_options, error, source_depth,
│                          # stack_guard, reference_directives, lib_global_types, scratchpad
├── tsox-types/            # the type system: arena, types, subtyping, namespace
│   └── src/               # symbol_map, type_annotation, type_display, explain, portable,
│                          # stable_walk, diagnostic_messages, semantic/ (generics, queries, solver)
├── tsox-model/            # the semantic model tables (node type, node symbol, flow facts,
│   └── src/               # constants, module info), arena-free and versioned
├── tsox-checker/          # the single-file checker
│   └── src/               # unit.rs (a file as a unit of work: UnitContext, UnitOutput, Worker)
│                          # bridge/ (parse, declare, imports, exports, narrow, expressions/,
│                          #          statements/, and the whole-file passes)
├── tsox-project/          # many files: module_resolution/ (graph, config, discovery, resolver,
│   └── src/               # check, schedule, partial), scc.rs. `module-resolution` feature
├── tsox-db/               # Salsa query layer: incremental.rs, incremental/project.rs
│   └── src/               # `incremental` feature
├── tsox-ir/               # a typed mid-level IR skeleton; nothing lowers into it yet
├── tsox-cli/              # the `tsox` command line (binary `tsox`) and its integration tests
├── tsox-lsp/              # the language server (`tsox-lsp`)
├── tsox-wasm/             # the browser front end (wasm-bindgen): `TsoxChecker`
├── tsox-emit/             # JavaScript emit over Oxc; reads the model for import elision
└── tsox-xtask/            # the `harness` and `compare-tsc` binaries, and `cargo xtask`

src/
└── lib.rs                 # the `tsox` facade: re-exports the public API of the crates above
                           # (the front ends reach the checker through this)

tests/                     # one integration test per milestone or feature
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
│                          # intersections, module-resolution, cross-file-types, the
│                          # project engine (scheduler, partial results, cycles, configuration,
│                          # library, query layer, safety), workspace, flow
├── language-coverage.md   # what the checker catches today, measured against tsc
├── conformance/           # the tsc conformance baseline
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
├── (layering, owners and corpus are `cargo xtask` commands: crates/tsox-xtask/src/bin/xtask/)
├── coverage-probes.py     # tsox against tsc on small probe programs; feeds docs/language-coverage.md
├── ts-diag-tool/          # tsc/tsgo diagnostic comparison, see README's own section on it
└── enable-hooks.sh

benches/                 # Criterion and IAI benchmarks
examples/                # Small usage and profiling examples
crates/                  # the workspace crates, including the CLI (see Repository structure)
```

```text
tests/corpus/             # the recorded corpus map (one JSON per part) and the known crashers
editors/                  # vscode/ and zed/ extensions for tsox-lsp
npm/                      # the npm package `tsox` (WebAssembly): package.json, types, smoke test
.github/workflows/        # ci.yml, auto-merge.yml, release.yml, docs.yml, codeql.yml, harness.yml
```

## Semantic milestones and regression contracts

Each semantic milestone maps directly to an executable regression suite. These are development contracts, not package release versions.

Start with the [documentation index](README.md), then inspect the relevant fixture directory and integration test together.

| Milestone | Test runner | Fixture directory | Documentation |
| --- | --- | --- | --- |
| Foundation | `tests/foundation.rs` | `tests/fixtures/foundation/` | [Foundation](foundation.md) |
| Structural Types | `tests/structural_types.rs` | `tests/fixtures/structural-types/` | [Structural Types](structural-types.md) |
| Literal Widening | `tests/literal_widening.rs` | `tests/fixtures/literal-widening/` | [Literal Widening](literal-widening.md) |
| Control-Flow Narrowing | `tests/control_flow_narrowing.rs` | `tests/fixtures/control-flow-narrowing/` | [Control-Flow Narrowing](architecture/narrowing.md) |
| Classes and Inheritance | `tests/classes_inheritance.rs` | `tests/fixtures/classes-inheritance/` | [Classes and Inheritance](classes-inheritance.md) |
| Expression and Program Checking | `tests/expression_program_checking.rs` | `tests/fixtures/expression-program-checking/` | [Expression and Program Checking](expression-program-checking.md) |
| Parameters and Destructuring | `tests/parameters_destructuring.rs` | `tests/fixtures/parameters-destructuring/` | [Parameters and Destructuring](parameters-destructuring.md) |
| Module Resolution | `tests/module_resolution.rs` (feature `module-resolution`) | `tests/module-resolution-fixtures/` | [Module Resolution](architecture/module-resolution.md) |
| Intersection Types | `tests/intersection_types.rs`, `tests/intersection_overloads.rs` | `tests/fixtures/intersection-types/`, `intersection-calls/`, `intersection-edges/` | [Intersection Types](architecture/intersections.md) |
| Cross-File Types | `tests/unit_exports.rs`, `tests/unit_imports.rs`, `tests/project_imports.rs`, `tests/import_diagnostics.rs` | sources inline in the tests | [Cross-File Types](architecture/cross-file-types.md) |
| Architecture Foundation | unit tests in `crates/tsox-db/src/incremental.rs`, `crates/tsox-base/src/scratchpad.rs` | none | [Architecture Foundation](architecture/foundation-dependencies.md) |
| Generics | `tests/generics_tier1.rs`, `tests/generics_tier2.rs`, `tests/generic_classes.rs`, `tests/recursive_types.rs`, `tests/builtin_generics.rs`, `tests/interface_methods.rs` | `tests/fixtures/generics-tier1/`, `generics-tier2/`, `generic-classes/`, `recursive-types/`, `builtin-generics/`, `interface-methods/` | [Generics](architecture/generics.md) |
