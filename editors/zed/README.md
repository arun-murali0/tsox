# tsox for Zed

Runs `tsox-lsp` for TypeScript, TSX and JavaScript. Install `tsox-lsp` and put it on your
`PATH`, then enable the language server in Zed's settings:

```json
{
  "languages": {
    "TypeScript": { "language_servers": ["tsox", "!typescript-language-server", "!vtsls", "..."] }
  }
}
```

To develop it: `zed: install dev extension` and pick this folder. Zed builds it to
`wasm32-wasip1`, which needs the Rust `wasm32-wasip1` target.
