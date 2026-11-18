# Contributing

Contributions are welcome: bug reports, failing test cases, fixes and new checks.

## What gets a pull request merged

Every branch and pull request runs three checks. All must pass.

| Check | What it runs | Run it yourself |
| --- | --- | --- |
| **All green** | `cargo fmt` check, the crate layering rule, the diagnostic-owner map, `cargo check --no-default-features`, `cargo test --workspace --all-features` | `bash scripts/ci.sh` |
| **Corpus map** | tsox over TypeScript's own test suite compared with the recorded map: nothing may get worse, and an improvement must update the map | [`cargo xtask corpus run && cargo xtask corpus verify`](corpus.md) |
| **Review** | The owner (`@Arun-murali0`) approves the pull request. | |

`main` stays clean: only changes that passed these checks reach it, by squash merge. Branches from
the owner's account are merged automatically once they pass and a staging gate (tsox against real
tsc on the fixtures) agrees. A pull request from anyone else is merged by the owner after review.

## A change, step by step

1. Fork, branch from `main`.
2. Write a small failing case first: a fixture in `tests/fixtures/<area>/` or a test in `tests/`.
3. Make the smallest change that fixes it, in the lowest crate that can hold it
   ([project structure](project-structure.md)).
4. `bash scripts/ci.sh` until it passes.
5. If you changed checker behaviour, run the corpus and update the map when results improved
   ([the corpus map](corpus.md)). Describe what changed in the pull request.
6. Open the pull request. Say what rule you implemented and which TypeScript behaviour it follows.

## Reporting a problem

The most useful report is the smallest source that shows a difference from `tsc`, with both outputs and
`tsox --version`. Crashes and hangs count as much as wrong diagnostics.

## Where things live

- Parser-facing code is in `crates/tsox-checker/src/bridge/`; the type system is in `crates/tsox-types`.
- Rules are registries (passes, narrowers, equality rules, call effects, repairs): adding one is
  adding a row to a table. See [registries](architecture/registries.md).
- Diagnostics have owners: [diagnostic owners](architecture/diagnostic-owners.md).
- The design notes are under [Architecture](architecture.md).
