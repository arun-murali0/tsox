#!/usr/bin/env bash
# Builds, tests and publishes the `tsox` npm package (WebAssembly) from this machine.
#
#   scripts/publish-npm.sh            dry run: builds, tests, packs, shows what would be published
#   scripts/publish-npm.sh --publish  really publishes
#
# The token is read from the environment or from `.env.development` (never committed; copy
# `.env.development.example`). It is written to a temporary .npmrc that is removed on exit.
set -euo pipefail
cd "$(dirname "$0")/.."

publish=0
[ "${1:-}" = "--publish" ] && publish=1

if [ -f .env.development ]; then
  set -a
  # shellcheck disable=SC1091
  . ./.env.development
  set +a
fi
tag="${NPM_TAG:-latest}"

bash scripts/build-wasm.sh >/dev/null
node npm/tests/smoke.js

cd npm/tsox
version=$(node -p "require('./package.json').version")
echo "package: tsox@$version (tag $tag)"
npm pack --dry-run 2>&1 | tail -15

if [ "$publish" -eq 0 ]; then
  echo "dry run only. Run with --publish to publish."
  exit 0
fi

if [ -z "${NPM_TOKEN:-}" ]; then
  echo "NPM_TOKEN is empty: set it in .env.development" >&2
  exit 1
fi
npmrc=$(mktemp)
trap 'rm -f "$npmrc"' EXIT
printf '//registry.npmjs.org/:_authToken=%s\n' "$NPM_TOKEN" > "$npmrc"
npm publish --access public --tag "$tag" --userconfig "$npmrc"
echo "published tsox@$version"
