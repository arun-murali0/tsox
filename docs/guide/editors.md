# Editors

Both extensions start `tsox-lsp`, so install it first ([Install](/guide/install)) and make sure
it is on your `PATH`.

The language server provides diagnostics, hover, completion, signature help, go to definition,
find references, rename, document highlight, outline, folding, selection ranges and workspace
symbols. Code actions, semantic tokens and inlay hints are not built yet.

## VS Code

Install the `.vsix` from the release (**Extensions: Install from VSIX…**). Set
`tsox.server.path` if `tsox-lsp` is not on your `PATH`. To avoid two sets of diagnostics, set
`"typescript.validate.enable": false` in the workspace.

## Zed

Install the extension from `editors/zed` with **zed: install dev extension**, then enable the
server for your languages:

```json
{
  "languages": {
    "TypeScript": { "language_servers": ["tsox", "!typescript-language-server", "!vtsls", "..."] }
  }
}
```

## Other editors

Any client that can launch `tsox-lsp --stdio` works. Logging goes to standard error;
set `TSOX_LSP_LOG=debug` to see more.
