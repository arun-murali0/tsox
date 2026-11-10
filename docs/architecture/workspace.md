# The workspace

tsox is a Cargo workspace of five library crates under `crates/` and a root package, `tsox`,
that is the public face of all of them. The split follows the layers the checker already had;
it moves code and adds boundaries, and changes no behavior. `cargo test --workspace
--all-features` passes with the same tests as before the split.

## Crates

| Crate | Owns | Depends on (tsox) | External |
|---|---|---|---|
| `tsox-base` | Identities and hashing (`keys`, `stable_hash`, `fxhash`), diagnostics and their codes, `FileId`, `LineIndex`, check options, the project-wide small helpers (`stack_guard`, `source_depth`, `reference_directives`), the generated list of tsc's global type names, the vendored TypeScript lib files (`lib/`, data only), the `scratchpad` | none | thiserror, xxhash-rust; serde (`wasm`), bumpalo (`scratchpad`) |
| `tsox-types` | The type system: `TypeArena` and `Type`, `subtyping`, generics inference (`semantic/`), `namespace`, `type_annotation`, `type_display`, `explain`, and `portable` (declarations as arena-free data, `ImportSource`) | base | oxc_ast, oxc_ast_visit, oxc_span, oxc_semantic, smallvec |
| `tsox-checker` | The single-file checker: `bridge/` (parse, bind, declare, imports, exports, narrowing, statements, expressions, whole-file passes) and `unit` (a file as a unit of work: `UnitContext`, `UnitOutput`, `Worker`) | base, types | oxc parser, allocator, cfg, semantic; tracing |
| `tsox-project` | Many files: `module_resolution/` (graph, config, discovery, resolver, check, schedule, partial results), `scc`. Feature `incremental` switches on the hooks the query layer needs | base, types, checker | oxc_resolver, rayon |
| `tsox-db` | The Salsa query layer over the project driver (`incremental`, `incremental/project.rs`) | base, types, checker, project | salsa |
| `tsox-cli` | The `tsox` command line: `src/main.rs` and its integration tests in `tests/` (`cli_project`, `corpus_aborts`, `depth_guard`, `determinism`, `partial_reasons`, `recursion_guards`). The binary is still named `tsox`, so `target/release/tsox` and `cargo build --bin tsox` are unchanged | `tsox` (facade only) | mimalloc (optional) |
| `tsox-lsp` | The language server: a stdio `tower` service with a worker that owns the incremental project, hover, navigation, symbols and diagnostics (see [lsp](lsp.md)) | `tsox` (facade, with its `incremental` feature), `tsox-base` | tokio, tower-lsp-server, oxc parser and semantic, tracing |
| `tsox-wasm` | The browser front end: `TsoxChecker`, a wasm-bindgen `cdylib` (`src/lib.rs`) | `tsox` (facade only, with its `wasm` feature) | wasm-bindgen, serde, serde-wasm-bindgen, console_error_panic_hook |
| `tsox-xtask` | The development tools: the `harness` and `compare-tsc` binaries (`src/bin/`) and `harness_launcher` | `tsox` (facade only) | xxhash-rust |
| `tsox` (root) | The facade: `src/lib.rs` re-exports the public API. Also the library's integration tests in `tests/`, the benches and the examples | base, types, checker, project, db | oxc crates, tracing |

## The rule

A crate depends only on crates above it in the table. The order is

```text
tsox-base < tsox-types < tsox-checker < tsox-project < tsox-db < tsox < tsox-cli | tsox-lsp | tsox-wasm | tsox-xtask
```

The three on the right are the front ends. They share a layer: each depends on the `tsox` facade and none on another.

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
| `wasm` | `tsox` | Serialize/Deserialize on the diagnostic types, via `tsox-base/wasm`. `tsox-wasm` turns it on |
| `module-resolution` (default), `incremental` | `tsox-cli` | The binary's own gates. Each one switches on the `tsox` feature of the same name, so the CLI code and its tests keep their `cfg(feature = ...)` |
| `mimalloc` | `tsox-cli` | mimalloc as the binary's global allocator: `cargo build --release -p tsox-cli --features mimalloc` |
| `test-hash-seed` | `tsox-base` | Exposes `fxhash::TEST_HASH_SEED` outside this crate's own tests. Only from a dev-dependency (`tsox-project` does) so its determinism tests can vary hash order |

`cfg(test)` is true only for the crate being tested. Anything a dependent crate's tests need
from a lower crate has to be a feature, not `cfg(test)`; the split first broke exactly this
(`TEST_HASH_SEED`, now fixed).

## The front-end crates

The CLI, the wasm front end and the tools used to live in the root package. They moved out so
the root is only the facade, and each front end depends on nothing but that facade.

- **Tests move with the binary they start.** Cargo sets `CARGO_BIN_EXE_<name>` only for the
  package that defines the binary, so the six CLI test files moved to `crates/tsox-cli/tests/`
  and `harness_launcher` to `crates/tsox-xtask/tests/`. Their test names are unchanged.
- **The repository root.** `harness` and `compare-tsc` read `.harness/` and `tests/` from the
  repository root, and `corpus_aborts` reads `tests/typescript-testcases/`. Each finds the root
  with `Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2)`: the crates sit two levels
  down, and `ancestors` keeps the path as `CARGO_MANIFEST_DIR` spells it, which `strip_prefix`
  on tool output relies on (a `../..` join would not).
- **`default-members`.** A workspace with a root package selects only that package by default.
  `default-members = [".", "crates/tsox-cli", "crates/tsox-xtask"]` keeps `cargo build`,
  `cargo build --release --bin tsox` and `cargo run --bin harness` working unchanged, which
  CI, `scripts/harness.sh` and `scripts/compare-local.sh` use. `tsox-wasm` is left out: it
  builds for a wasm target.
- **The wasm artifact is renamed.** The build now selects the crate, and its output is
  `tsox_wasm.wasm` (it was `tsox.wasm`):
  `cargo build --profile wasm-release -p tsox-wasm --target wasm32-unknown-unknown`, then
  `wasm-bindgen --target web --out-dir pkg target/wasm32-unknown-unknown/wasm-release/tsox_wasm.wasm`.
  A consumer that imports `tsox.js` from the bundle needs the new name.
- **The root library is an `rlib` only.** It was a `cdylib` too, for the wasm build; that
  artifact now comes from `tsox-wasm`.
- **`tests/` at the root** still holds the integration tests that go through the facade, and
  the shared fixtures and corpora the front-end tests also read.

## Commands

```text
cargo build                                   # the tsox CLI, harness and compare-tsc
cargo build --release -p tsox-cli --features mimalloc   # release CLI with mimalloc
cargo build --profile wasm-release -p tsox-wasm --target wasm32-unknown-unknown
cargo test --workspace --all-features         # every crate's tests and the integration tests
cargo check --workspace --no-default-features # every crate without the optional layers
python3 scripts/check-layering.py             # the dependency rule
bash scripts/ci.sh                            # the branch gate: fmt, layering, check, test
```

`--workspace` matters: a bare `cargo test` runs only the default members (the root, `tsox-cli`
and `tsox-xtask`), which would skip the unit tests in `tsox-base`, `tsox-types`, `tsox-checker`,
`tsox-project` and `tsox-db`.
