# tsox-cli

**Front end.** The `tsox` command line. Depends only on the `tsox` facade.

- `src/main.rs`: argument parsing, file and project discovery, `--project`, `--incremental`, `--watch`, `--tsc-codes`, and the report.
- `tests/`: the integration tests that start the binary (`CARGO_BIN_EXE_tsox`): `cli_project`, `corpus_aborts`, `depth_guard`, `determinism`, `partial_reasons`, `recursion_guards`. `corpus_aborts` reads the shared corpus in the repository's `tests/typescript-testcases/`.

The binary is named `tsox`, so `target/release/tsox` and `cargo build --bin tsox` work as before
(the workspace's `default-members` includes this crate).

Features: `module-resolution` (default) and `incremental` switch on the `tsox` features of the
same name; `mimalloc` sets the global allocator:
`cargo build --release -p tsox-cli --features mimalloc`.
