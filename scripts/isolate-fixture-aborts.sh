#!/usr/bin/env bash
# Run each TypeScript fixture in its own process so a stack overflow or abort cannot
# hide which input triggered it. Ordinary diagnostics/nonzero checker exit codes are
# expected and do not count as a crash.
set -uo pipefail

binary="${1:-target/release/tsox}"
fixture_root="${2:-tests/fixtures}"
if [[ ! -x "$binary" ]]; then
  echo "error: checker binary is not executable: $binary" >&2
  exit 2
fi
if [[ ! -d "$fixture_root" ]]; then
  echo "error: fixture directory does not exist: $fixture_root" >&2
  exit 2
fi

count=0
crashes=0
while IFS= read -r -d '' fixture; do
  count=$((count + 1))
  temp="$(mktemp -d)"
  relative="${fixture#"$fixture_root"/}"
  mkdir -p "$temp/$(dirname "$relative")"
  cp "$fixture" "$temp/$relative"
  printf '{}\n' > "$temp/tsconfig.json"

  if command -v timeout >/dev/null 2>&1; then
    timeout 30s "$binary" --project "$temp/tsconfig.json" \
      >/dev/null 2>"$temp/stderr"
    status=$?
  else
    "$binary" --project "$temp/tsconfig.json" >/dev/null 2>"$temp/stderr"
    status=$?
  fi

  if [[ "$status" -ge 128 || "$status" -eq 101 || "$status" -eq 124 || "$status" -eq 137 ]]; then
    crashes=$((crashes + 1))
    printf 'CRASH status=%s fixture=%s\n' "$status" "$fixture" >&2
    cat "$temp/stderr" >&2
  fi
  rm -rf "$temp"
done < <(find "$fixture_root" -type f \( -name '*.ts' -o -name '*.tsx' -o -name '*.mts' -o -name '*.cts' \) -print0 | sort -z)

printf 'checked=%s crashes=%s\n' "$count" "$crashes"
[[ "$crashes" -eq 0 ]]
