# The workspace

tsox is a Cargo workspace of five library crates under `crates/` and a root package, `tsox`,
that is the public face of all of them. The split follows the layers the checker already had;
it moves code and adds boundaries, and changes no behavior. `cargo test --workspace
--all-features` passes with the same tests as before the split.

## Crates

| Crate | Owns | Depends on (tsox) | External |
|---|---|---|---|
| `tsox-base` | Identities and hashing (`keys`, `stable_hash`, `fxhash`), diagnostics and their codes, `FileId`, `LineIndex`, check options, the project-wide small helpers (`stack_guard`, `source_depth`, `reference_directives`), the generated list of tsc's global type names, the `scratchpad` | none | thiserror, xxhash-rust; serde (`wasm`), bumpalo (`scratchpad`) |
| `tsox-types` | The type system: `TypeArena` and `Type`, `subtyping`, generics inference (`semantic/`), `namespace`, `type_annotation`, `type_display`, `explain`, and `portable` (declarations as arena-free data, `ImportSource`) | base | oxc_ast, oxc_ast_visit, oxc_span, oxc_semantic, smallvec |
| `tsox-checker` | The single-file checker: `bridge/` (parse, bind, declare, imports, exports, narrowing, statements, expressions, whole-file passes) and `unit` (a file as a unit of work: `UnitContext`, `UnitOutput`, `Worker`) | base, types | oxc parser, allocator, cfg, semantic; tracing |
| `tsox-project` | Many files: `module_resolution/` (graph, config, discovery, resolver, check, schedule, partial results), `scc`. Feature `incremental` switches on the hooks the query layer needs | base, types, checker | oxc_resolver, rayon |
| `tsox-db` | The Salsa query layer over the project driver (`incremental`, `incremental/project.rs`) | base, types, checker, project | salsa |
| `tsox-cli`, `tsox-wasm`, `tsox-xtask` | **Empty scaffolds**, ready to receive the CLI (`bin/tsox.rs`), the wasm front end (`src/wasm.rs`) and the tools (`bin/harness.rs`, `bin/compare-tsc.rs`). Each carries a comment saying what has to move with it | `tsox` (facade only) | none yet |
| `tsox` (root) | The facade (`src/lib.rs` re-exports the public API), the wasm front end (`src/wasm.rs`), the binaries in `bin/`, and the integration tests in `tests/` | all | the CLI and wasm dependencies |

## The rule

A crate depends only on crates above it in the table. The order is

```text
tsox-base < tsox-types < tsox-checker < tsox-project < tsox-db < tsox < tsox-cli | tsox-wasm | tsox-xtask
```

The three on the right share a layer: each depends on the `tsox` facade and none on another.

Dev-dependencies count. `scripts/check-layering.py` reads `cargo metadata`, fails on any
dependency that is not strictly downward, and fails when a crate has no layer. It runs first in
`scripts/ci.sh`, so a violation stops CI before anything compiles. To add a crate, put it in
`LAYERS` in that script and in the table above.

Two consequences hold the design together:

- **No `TypeId` crosses a unit.** A unit's output is the arena-free data in `portable`
  (`PortableDecl`, `ModuleExports`). That is why `tsox-project` and `tsox-db` can schedule and
  memoize units without knowing how the arena works, and why `tsox-db` needs from the checker only the
  `CancelToken` (it imports nothing from `bridge`).
- **The checker does not know about the driver or Salsa.** `tsox-checker` runs one unit;
  `tsox-project` decides the order and parallelism; `tsox-db` decides what has to run again.
  (See [query-layer](query-layer.md).)

Known limit: `tsox-types` depends on `oxc_ast` and `oxc_semantic` (symbol and node ids in the
arena and the annotation resolver). The type system is independent of the checker's traversal
code, not of Oxc.

## Features

Features live on the crate whose code they gate, and the root package forwards them.

| Feature | Defined on | Effect |
|---|---|---|
| `module-resolution` (default) | `tsox` | Pulls in `tsox-project`, `oxc_resolver`, `rayon`; the CLI's project mode |
| `incremental` | `tsox` | Pulls in `tsox-db` and `tsox-project/incremental`; `--incremental`, `--watch` |
| `scratchpad` | `tsox-base`, forwarded by `tsox` | Bump scratch for one worker |
| `wasm` | `tsox` | `TsoxChecker`; serde on the diagnostic types via `tsox-base/wasm` |
| `test-hash-seed` | `tsox-base` | Exposes `fxhash::TEST_HASH_SEED` outside this crate's own tests. Only from a dev-dependency (`tsox-project` does) so its determinism tests can vary hash order |

`cfg(test)` is true only for the crate being tested. Anything a dependent crate's tests need
from a lower crate has to be a feature, not `cfg(test)`; the split first broke exactly this
(`TEST_HASH_SEED`, now fixed).

## What stays in the root package, and why

- **`bin/` (the `tsox` CLI, `harness`, `compare-tsc`).** Seven integration test files start
  `tsox` and `harness` with `env!("CARGO_BIN_EXE_<name>")`, which Cargo sets only for binaries
  of the same package. Moving a binary to its own crate means moving those tests with it or
  rewriting how they find the binary; neither is a gain for a layering rule, which the
  binaries already follow.
- **`src/wasm.rs`.** The documented wasm build (`cargo build --profile wasm-release --lib
  --target wasm32-unknown-unknown --features wasm`) produces `tsox.wasm` from the root package.
  A separate `tsox-wasm` crate would change that artifact and the bundle layout.
- **`tests/`.** The integration tests exercise the whole stack through the facade. Tests of one
  layer alone live in that crate's `src/` as unit tests.

## Commands

```text
cargo build                                   # CLI with module resolution
cargo test --workspace --all-features         # every crate's tests and the integration tests
cargo check --workspace --no-default-features # every crate without the optional layers
python3 scripts/check-layering.py             # the dependency rule
bash scripts/ci.sh                            # the branch gate: fmt, layering, check, test
```

`--workspace` matters: `cargo test` at the root runs only the root package, which would skip
the unit tests that now live in the member crates.
