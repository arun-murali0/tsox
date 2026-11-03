# tsox-wasm

**Front end.** The browser build: `TsoxChecker` (`checkSource(source, fileName)`), a
wasm-bindgen `cdylib`. Depends only on the `tsox` facade, with its `wasm` feature (serde on the
diagnostic types). Positions are UTF-16 code units, as JavaScript and editors expect.

```text
cargo build --profile wasm-release -p tsox-wasm --target wasm32-unknown-unknown
wasm-bindgen --target web --out-dir pkg target/wasm32-unknown-unknown/wasm-release/tsox_wasm.wasm
```

The artifact is `tsox_wasm.wasm` (it was `tsox.wasm` when this lived in the root package).
It is not a default member of the workspace, since it needs a wasm target.
