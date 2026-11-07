#!/usr/bin/env bash
# Generates a seeded set of control-flow files and compares tsox with tsc on them.
#
#   scripts/flow-stress/run.sh [--seed N] [--files N] [--mode reach|assign] [--codes 7027,7029] [--keep DIR]
#
# Prints, per diagnostic code, how many lines agree and how many differ, then the first few
# differing lines with the statement around them. Exit status is 1 when anything differs.
set -euo pipefail
cd "$(dirname "$0")/../.."
seed=1; files=300; codes=""; keep=""; mode="reach"
while [ $# -gt 0 ]; do
  case "$1" in
    --seed) seed=$2; shift 2;;
    --files) files=$2; shift 2;;
    --codes) codes=$2; shift 2;;
    --keep) keep=$2; shift 2;;
    --mode) mode=$2; shift 2;;
    *) echo "unknown flag $1" >&2; exit 2;;
  esac
done
dir=${keep:-$(mktemp -d)}
rm -rf "$dir"; mkdir -p "$dir"
python3 scripts/flow-stress/gen.py "$dir" --seed "$seed" --files "$files" --mode "$mode"
NO_COLOR=1 TSOX_BIN=${TSOX_BIN:-target/debug/tsox} \
  scripts/compare-local.sh "$dir" --module --json --no-cache > "$dir.json" 2>/dev/null || true
python3 scripts/flow-stress/summarize.py "$dir.json" "$codes"
