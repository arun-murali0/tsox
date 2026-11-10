# tsox-lsp

The tsox language server, over stdio, built on tower. Everything runs in-process on tsox's own
parser, binder and checker.

```
cargo build -p tsox-lsp --release
target/release/tsox-lsp --stdio
```

Supported: diagnostics, document symbols, go to definition, find references, hover, file
watching. See [`docs/architecture/lsp.md`](../../docs/architecture/lsp.md) for how it is built,
what each phase added, and what is left to do.

Set `TSOX_LSP_LOG=debug` to see the server's logs on standard error (standard output carries
the protocol).

Tests: `cargo test -p tsox-lsp`. The end-to-end tests start the real binary and speak LSP to it
over its pipes.
