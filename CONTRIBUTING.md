# Contributing

## Workflow

1. Make a focused change.
2. Add or update regression coverage.
3. Run the repository quality script.
4. Review the diff for accidental files, secrets, and unrelated formatting churn.
5. Keep the semantic milestone name, tests, fixtures, and documentation aligned.
6. Open a pull request and let CI verify the same quality gates.

## What a merge needs

`main` only receives changes that passed **All green** (fmt, layering, tests) and **Corpus map**
(tsox over TypeScript's own test suite is not worse than the recorded map). Run both before you push:

```bash
./scripts/ci.sh
cargo xtask corpus fetch && cargo xtask corpus run && cargo xtask corpus verify
```

If results improved, `cargo xtask corpus update` records them; commit `tests/corpus/` in the same
pull request. The owner's branches merge automatically; a pull request from anyone else is reviewed
and merged by the owner. Details: `docs/contributing.md`, `docs/corpus.md`.

## AI assistance

Much of this code is drafted with AI assistance (Claude). That changes nothing about what a merge needs:
a person reads every change, and it must pass the same checks. If you use an AI assistant for a pull request,
you are responsible for the result. Read it, run the tests, and say so in the description. Design decisions
are made by the maintainer and recorded in [docs/decisions.md](docs/decisions.md); a change that goes against
one should say so and say why.

## Local checks

```bash
./scripts/ci.sh
```

The script formats the workspace, checks the crate layering (`cargo xtask layering`), then runs compilation and the full workspace test suite. The local pre-push hook uses the same sequence. Use `cargo test --workspace --all-features`, not a bare `cargo test`: that runs only the default members (the root, `tsox-cli`, `tsox-xtask`) and skips the unit tests in the layer crates.

## Code organization

Keep parser and Oxc-specific logic inside `crates/tsox-checker/src/bridge/`. Keep the semantic core independent from AST ownership details. Centralize compatibility rules in the subtype layer and keep generic algorithms in the semantic layer.

Use feature-oriented modules when a feature has a distinct reason to change. Do not split files merely to satisfy a line-count target.

Crates depend only downwards: `tsox-base < tsox-types < tsox-checker < tsox-project < tsox-db < tsox` (see `docs/architecture/workspace.md`). Put new code in the lowest crate that can hold it. Within the checker, the dependency direction should remain:

```text
Oxc AST → bridge feature → semantic service
```

Avoid making semantic services depend on AST traversal.

## Semantic milestones

Development milestones use descriptive capability names. Do not rename a milestone to an opaque numeric label. When adding a new milestone, add or update its:

- integration test;
- fixture directory;
- documentation page;
- entry in `docs/README.md`;
- entry in the roadmap when it becomes the active frontier.

## Tests

A semantic bug fix should normally include a regression fixture demonstrating the expected behavior. Prefer small fixtures that isolate one rule.

## Error handling

Do not use `unwrap()` as normal error handling. Prefer explicit propagation or an intentional fallback. Do not weaken strict Clippy rules simply to make CI pass.

## Comments and documentation

A comment stating what the next line of code obviously does is noise; a comment explaining why this approach was
chosen over an apparent alternative, what invariant it depends on, or what
would break if it were removed, is worth the space it takes. Most functions in
this codebase carry that kind of comment — that's intentional, not
accidental verbosity, and new code should match it rather than default to
sparser comments than its neighbors.

A useful test: if you deleted the comment, could a future contributor
re-derive the reasoning from the code alone? If not, it stays.

Put broader architectural reasoning, development history, and stage
boundaries in `docs/` rather than in a source comment that would otherwise
grow to several paragraphs — a source comment should explain the local
decision; a `docs/` page explains why the whole capability exists and how it
fits the rest of the checker.
