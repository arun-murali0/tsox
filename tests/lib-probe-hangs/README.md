# Hang repros (do not add to tests/fixtures or any folder run)

tsox does not return on these within 90 s. Run one at a time with a timeout:

    timeout 20 target/release/tsox --project <tsconfig listing the file>

- `l15_self_application_hang.ts`: three methods on one generic interface that return the interface
  with `T | U`, `T & U`, `T | U`. Any two of them are fast. Reduced from lib.es2025.collection.d.ts.
- `hang_corpus_nestedInfinitelyExpandedRecursiveTypes.ts`: from typescript-6.0.3 part-08.
