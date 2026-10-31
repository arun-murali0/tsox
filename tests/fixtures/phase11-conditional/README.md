# TSOX Phase 11: Conditional Types and `infer`

This focused fixture set covers conditional type reduction, distribution over unions, utility-type patterns, `infer` binding, constrained and repeated infer names, deferred conditionals, and edge cases involving `any`, `never`, `Promise`, `Record`, and infer scope.

## Contents

- `pass/`: 13 programs expected to compile with zero TypeScript diagnostics.
- `fail/`: 6 programs intentionally containing type errors.
- `manifest.json`: expected TypeScript pass/fail result for every fixture.
- `run-fixtures.py`: runs the TypeScript oracle and optionally TSOX, capturing output in `report.json`.

## Validate against TypeScript

From this directory:

```sh
python3 run-fixtures.py
```

By default the runner invokes:

```sh
tsc --noEmit --strict --target ES2022 --lib ES2022,DOM <fixture>
```

Set `TSC_CMD` to use a different TypeScript invocation. For example:

```sh
TSC_CMD='npx --yes --package typescript tsc --noEmit --strict --target ES2022 --lib ES2022,DOM' python3 run-fixtures.py
```

## Compare TSOX

The runner intentionally does not assume your TSOX CLI syntax. Provide a command template with `{file}` as the fixture placeholder:

```sh
TSOX_CMD='./target/release/tsox check {file}' python3 run-fixtures.py
```

Use the syntax supported by your build. The runner captures TSOX output and exit codes but does not interpret diagnostic codes or claim full conformance. Review the resulting `report.json` for false positives, missed errors, and unexpected messages.

## Applying source patches

- Apply `tsox-generic-phase11-conditionals-infer.patch` only on top of your Phase 1–10 source.
- Or apply `tsox-generic-phases-1-11.patch` to the pristine source tree.
- Do not apply both patches.

## Scope and known limitation

The implementation adds a symbolic conditional type node, branch reduction when types become concrete, naked-type-parameter distribution, common structural `infer` patterns, constrained infer handling, and initial `Promise`/`Record` application handling. It is not complete TypeScript conditional-type compatibility. In particular, infer-bearing conditional declarations are kept local by the current portable export path because the portable format does not yet encode scoped infer binders. Rust compilation and TSOX integration tests must be run in the project environment; the build toolchain was not available where this patch was prepared.
