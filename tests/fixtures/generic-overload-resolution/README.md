# Generic overload resolution fixtures

These are independent TypeScript oracle fixtures for Phase 13A.

- Seven fixtures must compile without diagnostics.
- `no_overload_matches.ts` must produce TS2769.
- `oracle.js` checks the expected diagnostic codes with the TypeScript compiler API.

Run from the Rust project root with TypeScript 5.8.3 installed and resolvable by Node:

```sh
node tests/fixtures/generic-overload-resolution/oracle.js
```

This oracle validates the expected TypeScript behavior only. It does not run TSOX.
