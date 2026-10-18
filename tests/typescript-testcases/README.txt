Source: microsoft/TypeScript tag v6.0.3, tests/cases/{compiler,conformance}
typescript-6.0.3/part-01 .. part-21   single-file .ts cases, 500 per folder (part-21 has 83); 10,083 total
known-crashers-6.0.3/                 8 cases that aborted tsox in project mode (out of memory or stack overflow).
                                      They no longer abort when run one at a time, and tests/corpus_aborts.rs runs
                                      them that way in every `cargo test`. Keep them out of folder runs.
Skipped: 1,983 multi-file (// @filename), 330 .tsx, 40 .d.ts

Check that nothing aborts (one file per process; fails on a signal, a panic exit or a timeout):
  cargo test --features module-resolution --test corpus_aborts
      one file in 500, plus the known crashers; this is what `cargo test` and CI run
  TSOX_CORPUS_STRIDE=1 cargo test --features module-resolution --test corpus_aborts
      all 10,083 files; about four minutes in a debug build
  TSOX_CORPUS_OFFSET=7 ...   shifts the sample, so a different slice of the corpus runs

Run one folder at a time to compare diagnostics:
  ./scripts/compare-local.sh tests/typescript-testcases/typescript-6.0.3/part-01

RUST_MIN_STACK is not needed: the checker's worker threads and the thread that reads a project's
imports each get a 64 MB stack. The checker stops recursing when a thread's stack budget is spent
(src/stack_guard.rs), and a file whose brackets nest past 4,000 levels is refused before it is
parsed.
