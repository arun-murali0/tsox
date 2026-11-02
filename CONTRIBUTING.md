# Contributing

## Workflow

1. Make a focused change.
2. Add or update regression coverage.
3. Run the repository quality script.
4. Review the diff for accidental files, secrets, and unrelated formatting churn.
5. Keep the semantic milestone name, tests, fixtures, and documentation aligned.
6. Open a pull request and let CI verify the same quality gates.

## Local checks

```bash
./scripts/ci.sh
```

The script formats the workspace, checks the crate layering (`scripts/check-layering.py`), then runs compilation and the full workspace test suite. The local pre-push hook uses the same sequence. Use `cargo test --workspace --all-features`, not a bare `cargo test`: the root package alone skips the unit tests that live in `crates/`.

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
