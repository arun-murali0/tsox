# tsox-types

**Layer 1.** The type system. Depends on `tsox-base`.

- `types`, `arena`: `Type` and the interning `TypeArena` (`TypeId`, `Ref` slots, `Named`/`App` nodes).
- `subtyping`: `is_subtype` and `is_assignable`, with `explain` for the reasons under a failure.
- `semantic/`: relation queries, generic inference (`generics`, `solver`).
- `namespace`, `symbol_map`, `type_annotation`, `type_display`, `diagnostic_messages`.
- `portable`, `stable_walk`: declarations as arena-free data, and the importer that rebuilds them in another arena. No `TypeId` crosses a unit boundary; this is the boundary.

Depends on `oxc_ast` and `oxc_semantic` for ids. It does not run the checker.
