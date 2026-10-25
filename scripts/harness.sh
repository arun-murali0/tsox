#!/usr/bin/env bash
# Compare tsox with tsc and tsgo on a real project: speed, memory, diagnostics.
# The program is bin/harness.rs; this script only builds the two binaries it needs and runs it.
#
#   ./scripts/harness.sh                       Zustand (cloned under .harness/zustand)
#   ./scripts/harness.sh /path/to/project      your own project (needs a tsconfig.json)
#   ./scripts/harness.sh --runs 10 --dhat      more timed runs, and the allocation profile
#   ./scripts/harness.sh --help                every option
#
# Results go to .harness/results/<UTC time>/ (report.md first).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

case " $* " in
  *" -h "* | *" --help "*) exec cargo run --quiet --release --bin harness -- --help ;;
esac

# The harness times the release build of tsox unless --tsox says otherwise, so make sure it is
# current; `cargo run` then builds the harness itself and honors CARGO_TARGET_DIR.
cargo build --release --bin tsox
exec cargo run --quiet --release --bin harness -- "$@"
