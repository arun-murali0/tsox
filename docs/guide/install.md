# Install

tsox is early software. It reports what it finds itself and can differ from `tsc`.

## Prebuilt binaries

Each release has archives for Linux (x86_64) and Windows (x86_64), with
two programs inside: `tsox` (command line) and `tsox-lsp` (language server). Unpack one and put the
folder on your `PATH`.

```sh
tsox --version
tsox-lsp --version
```

## From source

You need the Rust toolchain named by `rust-version` in the root `Cargo.toml`.

```sh
git clone https://github.com/arun-murali0/tsox
cd tsox
cargo install --path crates/tsox-cli
cargo install --path crates/tsox-lsp
```

## In JavaScript

See [npm (WebAssembly)](/guide/npm).
