#!/usr/bin/env bash
set -u
set -o pipefail

# Fast, local diagnostic comparison against real tsc -- for the whole
# tests/fixtures/ suite by default, or a narrower path when given one.
#
# All of the actual comparison logic (running tsc via the TypeScript
# compiler API, merging it against tsox's own output, rendering the
# MATCH/GAP/FALSE-POSITIVE table) lives in scripts/ts-diag-tool/compare.js.
# This script only builds tsox and invokes that.
#
# Why the compiler API instead of shelling `tsc` and text-parsing its
# output: a type name can itself contain parens or colons, which broke the
# old awk-based line parser. ts.Diagnostic gives code/category/message as
# real structured fields -- see scripts/ts-diag-tool/check-fixtures.js.
#
# Unlike crates/tsox-xtask/src/bin/compare-tsc.rs (which is in-process, errors-only, and scoped to
# the small dedicated tests/tsc-conformance/ set), this script:
#   - defaults to every .ts file under tests/fixtures/ -- the real, full
#     feature-tier suite, not a curated subset
#   - is informational, not pass/fail: many fixtures under tests/fixtures/
#     deliberately exercise this checker's OWN recovery behavior (e.g. one
#     unresolvable class member making the whole class unsupported) and are
#     expected to diverge from tsc -- this never exits nonzero for that
#
# Usage:
#   ./scripts/compare-local.sh                        # all of tests/fixtures
#   ./scripts/compare-local.sh tests/fixtures/generics-tier1
#   ./scripts/compare-local.sh path/to/one_file.ts
#   ./scripts/compare-local.sh tests/fixtures --strict-only   # old flag set
#   ./scripts/compare-local.sh tests/fixtures --json          # machine-readable
#   ./scripts/compare-local.sh tests/fixtures --summary       # numbers only, no tables or JSON
#   ./scripts/compare-local.sh tests/fixtures --background    # starts detached, returns at once
#   ./scripts/compare-local.sh --status [run-folder]          # state, progress and the summary
#   ./scripts/compare-local.sh path/to/project --module       # cross-file: tsc checks the
#                                                             # directory as ONE project
#   ./scripts/compare-local.sh tests/fixtures --tsc --summary  # use real tsc as the reference
#
# The reference is tsgo (the native TypeScript 7 compiler) by default. It is designed to
# report the same diagnostics as TypeScript 6.0 and is several times faster, so a whole-suite
# comparison finishes in seconds. It checks the directory as ONE project, like --module, so
# it needs a directory target: a single-file target falls back to tsc. Pass --tsc to compare
# against tsc (the TypeScript 6.0 compiler API, one isolated Program per file, with its
# result cache) instead. The time and peak memory shown for the reference are its own. The
# first run installs typescript@7 under scripts/ts-diag-tool/.tsgo when no tsgo is found
# (see TSGO_BIN below). The report keys stay `tsc_*` (they name the reference side);
# `reference:` says which compiler it was. CI calls scripts/ts-diag-tool/compare.js directly,
# which still defaults to tsc unless given --tsgo.
#
# --module is for projects whose files import each other. tsox checks them through its
# module graph, so tsc has to as well to give a fair time and peak memory: it checks every
# file of the directory in one Program, in its own process, instead of one isolated Program
# per file (which is right for tests/fixtures, where files reuse names, and stays the
# default). The target must be a directory. Every file is its own module for tsc, so it also
# works on tests/fixtures, where it gives the same diagnostics as the per-file mode.
#
# Environment:
#   TSOX_BIN    tsox command override (default: target/release/tsox,
#                  falls back to a debug build if release is missing)
#   TSGO_BIN    tsgo command for --tsgo (default: tsgo on PATH, else the copy this script
#                  installs under scripts/ts-diag-tool/.tsgo; TSGO_VERSION picks the
#                  typescript version to install, default 7)
#   NO_COLOR       set to disable colored output (also auto-disabled when
#                  stdout is not a terminal)

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}" || exit 2

# --status only reads a background run's folder, so it needs neither the build nor tsc.
if [ "${1:-}" = "--status" ]; then
  exec node scripts/ts-diag-tool/compare.js "$@"
fi

# A first argument that is a flag leaves the target at its default.
if [[ "${1:-}" == --* ]]; then
  TARGET="tests/fixtures"
else
  TARGET="${1:-tests/fixtures}"
  shift || true
fi
EXTRA_ARGS=("$@")

DIAG_TOOL_DIR="scripts/ts-diag-tool"

# Reinstall when typescript is missing or is not the version package.json pins, so a checkout
# that still has an older typescript in node_modules is brought up to date by itself.
want_ts="$(node -p "require('./${DIAG_TOOL_DIR}/package.json').dependencies.typescript" 2>/dev/null)"
have_ts="$(node -p "require('./${DIAG_TOOL_DIR}/node_modules/typescript/package.json').version" 2>/dev/null)"
if [ "${want_ts}" != "${have_ts}" ]; then
  echo "Installing scripts/ts-diag-tool dependencies (typescript@${want_ts}, pinned -- see its package.json)..." >&2
  (cd "${DIAG_TOOL_DIR}" && npm install --no-audit --no-fund) || {
    echo "error: npm install failed in ${DIAG_TOOL_DIR}" >&2
    exit 2
  }
fi

# --tsgo: make sure a tsgo binary exists before the run, installing typescript@7 (which ships
# the native compiler as an optional platform package) into scripts/ts-diag-tool/.tsgo.
# tsgo is the default reference; --tsc turns it off, and a single-file target cannot use it
# (tsgo checks a directory as one project), so that case stays on tsc.
wants_tsgo=1
args_out=()
for a in ${EXTRA_ARGS[@]+"${EXTRA_ARGS[@]}"}; do
  case "$a" in
    --tsc) wants_tsgo=0 ;;
    --tsgo) ;;
    *) args_out+=("$a") ;;
  esac
done
EXTRA_ARGS=(${args_out[@]+"${args_out[@]}"})
if [ "${wants_tsgo}" = 1 ] && [ -f "${TARGET}" ]; then
  echo "note: ${TARGET} is a single file, so the reference is tsc (tsgo needs a directory)." >&2
  wants_tsgo=0
fi
[ "${wants_tsgo}" = 1 ] && EXTRA_ARGS+=(--tsgo)
if [ "${wants_tsgo}" = 1 ] && [ -z "${TSGO_BIN:-}" ] && ! command -v tsgo >/dev/null 2>&1; then
  tsgo_dir="${DIAG_TOOL_DIR}/.tsgo"
  if [ -z "$(find "${tsgo_dir}/node_modules/@typescript" -path '*/lib/tsc' -type f 2>/dev/null | head -1)" ]; then
    echo "Installing tsgo (typescript@${TSGO_VERSION:-7}) under ${tsgo_dir}..." >&2
    mkdir -p "${tsgo_dir}"
    (cd "${tsgo_dir}" && npm install --no-audit --no-fund --no-save --prefix . "typescript@${TSGO_VERSION:-7}") || {
      echo "error: could not install tsgo; set TSGO_BIN to an existing binary" >&2
      exit 2
    }
  fi
fi

if [ -z "${TSOX_BIN:-}" ]; then
  if [ -x "target/release/tsox" ]; then
    TSOX_BIN="target/release/tsox"
  else
    echo "No release build found; building target/release/tsox..." >&2
    cargo build --release --bin tsox || {
      echo "error: cargo build failed" >&2
      exit 2
    }
    TSOX_BIN="target/release/tsox"
  fi
fi

if [ ! -x "${TSOX_BIN}" ]; then
  echo "error: tsox binary not found or not executable at ${TSOX_BIN}" >&2
  exit 2
fi

if [ ! -e "${TARGET}" ]; then
  echo "error: no such file or directory: ${TARGET}" >&2
  exit 2
fi

exec node "${DIAG_TOOL_DIR}/compare.js" "${TARGET}" "${TSOX_BIN}" "${EXTRA_ARGS[@]}"
