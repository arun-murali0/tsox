#!/usr/bin/env bash
set -euo pipefail
TSC_BIN="${TSC_BIN:-tsc}"
expected=0
actual=0
for file in pass/*.ts; do
  expected=$((expected + 1))
  if "$TSC_BIN" --noEmit --strict --skipLibCheck --target ES2020 "$file" >/tmp/tsox-phase13-tsc.out 2>&1; then
    echo "PASS oracle: $file"
    actual=$((actual + 1))
  else
    echo "FAIL oracle: expected TypeScript success for $file"
    cat /tmp/tsox-phase13-tsc.out
  fi
done
for file in fail/*.ts; do
  expected=$((expected + 1))
  if "$TSC_BIN" --noEmit --strict --skipLibCheck --target ES2020 "$file" >/tmp/tsox-phase13-tsc.out 2>&1; then
    echo "FAIL oracle: expected TypeScript error for $file"
  else
    echo "PASS oracle: expected error for $file"
    actual=$((actual + 1))
  fi
done
echo "Oracle outcomes: $actual/$expected expected"
[[ "$actual" -eq "$expected" ]]
