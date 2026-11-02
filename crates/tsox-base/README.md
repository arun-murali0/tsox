# tsox-base

**Layer 0.** Identities and hashing, diagnostics, and the small helpers every other tsox crate shares. Depends on no other tsox crate.

- `keys`, `stable_hash`: `ModuleKey`, `DeclKey`, `SigHash`, `NodeHash`, and the canonical encoding they hash.
- `diagnostics`, `diagnostic_codes`, `diagnostic_view`: the `Diagnostic` type, tsc codes, and the position-converted view.
- `file_id`, `line_index`, `check_options`, `error`, `project`.
- `fxhash`, `stack_guard`, `source_depth`, `reference_directives`, `lib_global_types` (generated), `scratchpad`.

Features: `scratchpad` (bump scratch), `wasm` (serde on diagnostics), `test-hash-seed` (see
`docs/architecture/workspace.md`; enable it only from a dev-dependency).

Rule: nothing here may import a higher tsox crate. `scripts/check-layering.py` enforces it.
