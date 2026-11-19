# tsox for VS Code

Diagnostics, hover, completion, signature help, go to definition, references, rename, outline,
folding and workspace symbols for TypeScript and JavaScript, served by `tsox-lsp`: the tsox
checker running in-process, with no Node and no `tsc`.

## Setup

1. Install `tsox-lsp` (a release archive from the repository, or `cargo install --path crates/tsox-lsp`)
   and put it on your `PATH`, or set **tsox.server.path** to the executable.
2. Open a folder with a `tsconfig.json`.
3. To avoid two sets of diagnostics, set `"typescript.validate.enable": false` in the workspace.

Commands: **tsox: Restart Language Server**, **tsox: Show Output**.

tsox is young. It reports what it finds itself and may differ from `tsc`; please report
differences at https://github.com/tsoxide/tsox/issues.
