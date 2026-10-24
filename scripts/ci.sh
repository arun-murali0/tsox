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

# The module-resolution feature is on by default, so every other step builds with it. A
# public item exported without its gate only fails to build here.
echo "==> cargo check --no-default-features"
cargo check --locked --no-default-features

# --locked: a stale Cargo.lock fails instead of being rewritten quietly.
echo "==> cargo test --features module-resolution"
cargo test --locked --all-features

echo "==> CI PASSED"
