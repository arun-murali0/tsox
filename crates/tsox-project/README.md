# tsox-project

**Layer 3.** Many files. Depends on `tsox-base`, `tsox-types` and `tsox-checker`.

- `module_resolution/`: `ModuleGraph::build` (oxc_resolver and tsconfig), `check_project`, the ready-queue scheduler (`run_units`), the second pass, partial results.
- `scc`: iterative Tarjan; components come dependencies first.

Feature `incremental` exposes the hooks the query layer needs (check one unit, the memo); the
Salsa dependency itself is in `tsox-db`. Results are keyed by `FileId` and ordered, so the
report does not depend on thread timing.
