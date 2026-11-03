# tsox-xtask

**Front end.** The development tools. Depends on the `tsox` facade.

- `harness` (`src/bin/harness.rs`): compares tsox with `tsc` and `tsgo` on one real project and writes a report under `.harness/`. Start it with `scripts/harness.sh`; it runs the `tsox` binary, so it needs `cargo build --bin tsox` first.
- `compare-tsc` (`src/bin/compare-tsc.rs`): in-process, errors-only comparison with `tsc` on `tests/tsc-conformance/`.
- `tests/harness_launcher.rs`: starts the harness binary (`CARGO_BIN_EXE_harness`).

Both binaries find the repository root two levels above this crate.
