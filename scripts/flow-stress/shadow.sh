#!/usr/bin/env bash
# Runs the backward flow resolver in shadow mode over generated narrowing code and reports where
# it and the forward overlay give a read different types. No tsc involved: this compares the two
# implementations with each other.
#
#   scripts/flow-stress/shadow.sh [--seed N] [--files N] [--mode narrow|narrow-try] [--show N]
set -euo pipefail
cd "$(dirname "$0")/../.."
seed=1; files=100; mode=narrow; show=6
while [ $# -gt 0 ]; do
  case "$1" in
    --seed) seed=$2; shift 2;;
    --files) files=$2; shift 2;;
    --mode) mode=$2; shift 2;;
    --show) show=$2; shift 2;;
    *) echo "unknown flag $1" >&2; exit 2;;
  esac
done
dir=$(mktemp -d)
python3 scripts/flow-stress/gen.py "$dir" --seed "$seed" --files "$files" --mode "$mode"
echo '{"compilerOptions":{"strict":true}}' > "$dir/tsconfig.json"
TSOX_FLOW_SHADOW=all "${TSOX_BIN:-target/debug/tsox}" --project "$dir/tsconfig.json" > "$dir.out" 2>&1 || true
python3 scripts/flow-stress/shadow_summary.py "$dir" "$dir.out" "$show" || true
python3 scripts/flow-stress/three_way.py "$dir" "$dir.out" "$show"
