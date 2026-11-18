#!/usr/bin/env bash
# Builds the npm package `tsox` (WebAssembly): the wasm artifact, then wasm-bindgen glue for
# Node and for browsers/bundlers, into npm/tsox/{node,web}.
#
# Needs: the rust target wasm32-unknown-unknown, and wasm-bindgen-cli at the same version as
# the `wasm-bindgen` crate in Cargo.lock (`cargo install wasm-bindgen-cli --version X --locked`).
set -euo pipefail
cd "$(dirname "$0")/.."

want=$(awk '/name = "wasm-bindgen"$/{getline; gsub(/[version = "]/,""); print; exit}' Cargo.lock)
have=$(wasm-bindgen --version 2>/dev/null | awk '{print $2}' || true)
if [ "$want" != "$have" ]; then
  echo "wasm-bindgen-cli is '$have', Cargo.lock wants '$want'." >&2
  echo "  cargo install wasm-bindgen-cli --version $want --locked" >&2
  exit 1
fi

cargo build --profile wasm-release -p tsox-wasm --target wasm32-unknown-unknown
wasm=target/wasm32-unknown-unknown/wasm-release/tsox_wasm.wasm
rm -rf npm/tsox/node npm/tsox/web
wasm-bindgen --target nodejs --out-dir npm/tsox/node --out-name tsox "$wasm"
wasm-bindgen --target web    --out-dir npm/tsox/web  --out-name tsox "$wasm"
if command -v wasm-opt >/dev/null; then
  for dir in node web; do wasm-opt -Oz -o "npm/tsox/$dir/tsox_bg.wasm" "npm/tsox/$dir/tsox_bg.wasm"; done
fi
# wasm-bindgen writes a package.json and .gitignore in each out dir; the package has its own.
rm -f npm/tsox/node/package.json npm/tsox/web/package.json npm/tsox/node/.gitignore npm/tsox/web/.gitignore
# The web build is an ES module; the package itself is CommonJS (the Node build).
echo '{ "type": "module" }' > npm/tsox/web/package.json
cp LICENSE-MIT LICENSE-APACHE npm/tsox/
ls -la npm/tsox/node npm/tsox/web
