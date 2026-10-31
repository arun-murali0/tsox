# tsox Generic Regression Stress Suite

This suite contains 42 standalone TypeScript fixtures covering generic inference and nearby type-system behavior across the work in Phases 1–10.

- `pass/`: each file should compile with **zero diagnostics** under TypeScript.
- `fail/`: each file intentionally contains at least one type error under TypeScript.
- `manifest.json`: expected TypeScript outcome and a short description for each fixture.
- `run-fixtures.py`: runs `tsc` and an optional local `tsox` command, then writes a JSON report.

## Run TypeScript oracle

From this directory:

```bash
npx --yes --package typescript tsc --noEmit --strict --target ES2022 --lib ES2022,DOM pass/*.ts
```

The `fail/` directory is expected to fail. Check each fixture individually so one failure does not hide the rest:

```bash
for f in pass/*.ts fail/*.ts; do
  npx --yes --package typescript tsc --noEmit --strict --target ES2022 --lib ES2022,DOM "$f"
  rc=$?
  case "$f:$rc" in
    pass/*:0|fail/*:1) echo "OK $f" ;;
    *) echo "UNEXPECTED $f (exit $rc)" ;;
  esac
done
```

Shells differ in how they expand and report these globs; the Python runner below is the more reliable option.

## Run both checkers and collect evidence

Set `TSOX_CMD` to your actual tsox invocation, using `{file}` as the file placeholder. This avoids guessing your CLI syntax.

Example if your binary accepts a file path directly:

```bash
TSOX_CMD='./target/release/tsox {file}' python3 run-fixtures.py
```

If your CLI uses a subcommand, provide it in the environment, for example `TSOX_CMD='./target/release/tsox check {file}'`.

The runner records each command's exit status and combined output in `report.json`. It classifies TSOX's result only by whether output/exit status indicates diagnostics, so review any mismatches manually. The TypeScript oracle is the authoritative expected pass/fail classification.

## Important interpretation notes

- This is a regression/stress corpus, not a claim that every fixture has already been run against your modified build.
- `fail/` fixtures are intentionally invalid; the test passes when the checker rejects them, not when it accepts them.
- A diagnostic-count match is not sufficient by itself. Inspect diagnostic codes and locations, especially for generic inference false positives.
- If your fixture harness requires special naming or golden files, adapt the manifest to that convention rather than silently treating a CLI exit code as full conformance.
