#!/usr/bin/env bash
set -euo pipefail

# Branch gate, same locally and in CI: `bash scripts/ci.sh`. Only fmt and tests.
# CI sets RUSTFLAGS="-D warnings" in the workflow; it is not exported here so a local
# run keeps your own rustflags (e.g. mold) and build cache.

# Locally this rewrites files so formatting never blocks a commit. In CI (CI=true) it
# only checks: formatting a throwaway checkout would let unformatted code pass.
echo "==> cargo fmt"
if [ -n "${CI:-}" ]; then
  cargo fmt --all -- --check
else
  cargo fmt --all
fi

# The crates only depend downwards (base < types < checker < project < db < tsox). Reads
# `cargo metadata`, so it needs no build.
echo "==> layering"
python3 scripts/check-layering.py

# The module-resolution feature is on by default, so every other step builds with it. A
# public item exported without its gate only fails to build here.
echo "==> cargo check --workspace --no-default-features"
cargo check --locked --workspace --no-default-features

# --locked: a stale Cargo.lock fails instead of being rewritten quietly. --workspace: a bare
# `cargo test` runs only the default members (the root, tsox-cli, tsox-xtask) and would skip the
# unit tests in the layer crates and the wasm crate.
echo "==> cargo test --workspace --all-features"
cargo test --locked --workspace --all-features

echo "==> CI PASSED"
