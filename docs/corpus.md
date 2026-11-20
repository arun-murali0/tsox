# The corpus map

tsox is measured against TypeScript's own test suite: every case of
`tests/cases/compiler` and `tests/cases/conformance` at the release tag **v6.0.3**. That is 10,083
single-file cases in 21 parts of 500 (`part-01` … `part-21`), and 2,313 multi-file or `.tsx` cases in 10
project parts of 250 (`part-p01` … `part-p10`), 14,274 checked files in all. The suite is not stored in this repository. It is fetched by tag, and what tsc and
tsox say about every file is recorded as data, so a change is checked in seconds instead of by running
tsc over 10,000 files again.

## The three pieces

| Piece | Where | What it is |
| --- | --- | --- |
| The suite | `.corpus/` (git ignores it) | Downloaded from `microsoft/TypeScript` at the tag by `cargo xtask corpus fetch`, sorted, split into the parts above, line endings turned into LF. The same bytes on every machine. |
| The map | `tests/corpus/typescript-6.0.3/part-NN.json` | One entry per file: the verdict, and for every line with a diagnostic on either side what tsc said and what tsox said (code and message). Clean files are `MATCH` with no lines. |
| The known crashers | `tests/corpus/known-crashers/` | Eight cases that once aborted tsox. They stay in the repository and run one at a time in every `cargo test`. |

The map keeps more than a count. A line that changes its code or its wording, a diagnostic that
appears or disappears, shows up as a change in the file's entry.

## Multi-file and JSX cases

A case with more than one `// @filename:` section, and every `.tsx` case, is split the way tsc's own
test harness splits it: each `// @name: value` line is an option and belongs to no file, and each
`// @filename: x` starts the file `x`. The case becomes a folder (`part-p03/<case>/…`) and is checked as one
project, so imports, `export *`, ambient modules and a file's own globals are exercised across files. Entries
in the map are keyed `case/relative/path`, because two cases can both have an `a.ts`.

tsox checks each case as a project of its own, with the same options as for single files, and tsc is
run once per case when the map is recorded:

```sh
cargo xtask corpus run --real --parts p01,p02    # record tsc's answers for project parts (slow: about a minute per 25 cases)
cargo xtask corpus build <report-dir>            # write the map from those reports
```

The options in a case's `// @name: value` lines (`@target`, `@jsx`, `@module` …) are not applied for
single-file or project cases alike: both tools run with the harness defaults, so the numbers say how
close tsox is to tsc under one configuration, not under each case's own.

## Everyday use

```sh
cargo build --release --bin tsox
cargo xtask corpus fetch      # once; later runs reuse .corpus/
cargo xtask corpus run        # tsox over all 21 parts, tsc's answers read from the map
cargo xtask corpus verify     # compare the run with the map
./scripts/compare-local.sh corpus:part-03 --summary   # one part, with the per-file tables (corpus:all does run + verify)
```

A whole run takes seconds, because only tsox runs. `--parts 03,07` limits it to some parts.

`verify` sorts every file into **same**, **better** (more exact lines, fewer gaps, fewer false
positives), **changed** (different but not better or worse) or **worse** (a line that matched no
longer does, a new false positive, a file that stopped matching), and exits with:

| Exit code | Meaning |
| --- | --- |
| 0 | The map and the run agree. |
| 1 | Something got worse. Fix it, or explain in the pull request why it is intended. |
| 3 | No regression, but the map is out of date. Run `update` and commit `tests/corpus/`. |

## When your change improves things

```sh
cargo xtask corpus run
cargo xtask corpus update     # rewrites the map entries from this run
git add tests/corpus && git commit
```

The diff of the map is the evidence: reviewers see exactly which files started matching. CI fails a
pull request whose map is stale in either direction, so the map on `main` is always the truth.

## Rebuilding the map against real tsc

Only needed when the corpus tag or the reference compiler changes.

```sh
cd scripts/ts-diag-tool && npm ci && cd ../..
for p in $(seq -w 1 21); do
  scripts/compare-local.sh .corpus/typescript-6.0.3/part-$p --tsc --json > /tmp/reports/part-$p.json
done
cargo xtask corpus build /tmp/reports
```

That takes tens of minutes, since real tsc runs over every file. To move to a new TypeScript release,
pass `--tag vX.Y.Z` to every command and add the new map folder beside the old one.

## What the verdicts mean

| Verdict | Meaning |
| --- | --- |
| `MATCH` | tsox and tsc report the same diagnostics (or none) on every line. |
| `GAP` | tsc reports something tsox misses. Expected while the checker grows. |
| `FALSE POSITIVE` | tsox reports something tsc does not. The damaging kind: valid code rejected. |
| `CODE DIFFERS` / `MESSAGE DIFFERS` | Same line, different error code or different wording. |
| `MIXED` | A mix of the above across lines. |

## In CI

The **Corpus map** check in `ci.yml` builds tsox, restores the fetched corpus from the Actions cache
(keyed by the tag), runs `run` and `verify`, and prints the result in the job summary. It is one of the
checks a pull request needs before it can reach `main`.
