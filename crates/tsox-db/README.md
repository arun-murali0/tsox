# tsox-db

**Layer 4.** The Salsa query layer over the project driver. Depends on `tsox-base`, `tsox-types`, `tsox-checker` and `tsox-project`.

`IncrementalProject` keeps each unit's result so a later check runs only what an edit reaches:
`SourceFile` inputs with content hashes, `module_header`, `decl_shape`, `export_names`,
per-component memos with an LRU, and cooperative cancellation. It returns the same
`ProjectReport` as `check_project`. Design and measurements: `docs/architecture/query-layer.md`.
