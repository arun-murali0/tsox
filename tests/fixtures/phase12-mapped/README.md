# TSOX Phase 12 mapped utility fixtures

Five expected-pass and five expected-fail TypeScript fixtures for `Partial`, `Required`, `Pick`, and `Omit`.

Run each file independently with:

```sh
npx --yes --package typescript tsc --noEmit --strict --target ES2022 "$file"
```

Phase 12 reduces these four utility types to ordinary object shapes. `Readonly`, arbitrary mapped type syntax (`{ [K in keyof T]: ... }`), key remapping with `as`, and readonly write checking are not implemented in this phase. They are intentionally excluded from the expected-pass set.
