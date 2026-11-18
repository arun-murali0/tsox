# tests/corpus

The recorded comparison of tsox with `tsc` over TypeScript's own test suite. How to use it:
[docs/corpus.md](../../docs/corpus.md) (`cargo xtask corpus`).

- `typescript-6.0.3/part-NN.json`: the map. One entry per file of the corpus (the single-file `.ts`
  cases of `tests/cases/{compiler,conformance}` at tag v6.0.3, 500 per part): the verdict and, per
  line, what tsc and tsox said. The suite itself is fetched into `.corpus/`, not stored here.
- `known-crashers/`: eight cases that once aborted tsox in project mode (out of memory or stack
  overflow). They no longer abort when run one at a time, and `crates/tsox-cli/tests/corpus_aborts.rs`
  runs them that way in every `cargo test`. They are kept out of the folder runs.
- `known-crashers.txt`: their names, so `cargo xtask corpus fetch` leaves them out of the parts.

The crasher files are unmodified test cases from https://github.com/microsoft/TypeScript, which is
under the Apache License 2.0. Copyright (c) Microsoft Corporation.

Check that nothing aborts (one file per process; fails on a signal, a panic exit or a timeout):

    cargo test -p tsox-cli --test corpus_aborts              # one file in 500, plus the crashers
    TSOX_CORPUS_STRIDE=1 cargo test -p tsox-cli --test corpus_aborts   # all 10,083 files
    TSOX_CORPUS_OFFSET=7 ...                                 # shifts the sample

`TSOX_CORPUS_DIR` points the tests at a corpus somewhere other than `.corpus/`;
`TSOX_REQUIRE_CORPUS=1` makes a missing corpus a failure instead of a skip (CI sets it).
