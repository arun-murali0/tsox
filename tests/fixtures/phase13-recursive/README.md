# TSOX Phase 13: Recursive types

Each fixture is compiled independently with TypeScript 5.8.3. Files in `pass/` must compile; files in `fail/` must produce a TypeScript error.

Run the oracle checks from this directory:

```sh
./check-oracle.sh
```

The oracle checks confirm the test expectations against TypeScript. They are not a substitute for running the TSOX Rust integration suite against the same cases.
