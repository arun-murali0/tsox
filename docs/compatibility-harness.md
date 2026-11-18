# Compatibility harness and fixture comparison

`scripts/harness.sh` compares tsox with `tsc` and `tsgo` on one real project and writes a report you can check by hand. The program is `crates/tsox-xtask/src/bin/harness.rs`.

It measures:

- **Speed:** wall time of each compiler after warm-up runs, with the runs interleaved so a slow minute hurts all three alike. Median, mean, standard deviation, min, max, quartiles and a noise flag.
- **Memory and CPU:** peak resident memory and CPU time of each process. Each program is started by a small measuring process, because the kernel otherwise counts the parent's size as the child's.
- **Agreement:** tsox's errors against tsc's and tsgo's, matched by file, line and tsc error code. It counts matches, false positives (tsox errors the reference lacks), gaps (reference errors tsox misses), lines that agree, and files by class. tsc and tsgo are also compared with each other, to show whether the reference itself is consistent.
- **Coverage:** what tsox skipped. Every `TSR9xxx` warning is a construct it did not check, and a partial export is a file whose exports are incomplete. The report says how many agreeing files tsox fully checked, since agreement on a skipped file is weak evidence.

**It is not a semantic-equivalence oracle.** A matching result does not prove two compilers implement the same TypeScript semantics, and tsox checks a smaller language than tsc, so a speed ratio only means something next to the agreement numbers.

### Running it locally

```bash
./scripts/harness.sh
```

This clones Zustand (by default), installs its dependencies with its own lockfile, installs tsc and tsgo under `.harness/tools`, builds `tsox` in release mode, and runs everything. Open `.harness/results/<UTC time>/report.md`.

Each results folder holds `report.md`, `summary.json`, `per-file.csv`, `tsox-false-positives.txt`, `tsox-missed.txt`, `tsc-vs-tsgo-differences.txt`, `REPRODUCE.sh` with the exact commands, and `raw/` with the output and `timings.csv` of every run.

Common variations:

```bash
# Your own project instead of Zustand (needs a tsconfig.json)
./scripts/harness.sh /path/to/your/ts/project

# More timed runs for steadier medians
./scripts/harness.sh --runs 10

# Also run the allocation profile of the benchmark workloads (examples/dhat_heap)
./scripts/harness.sh --dhat

# A pinned Zustand revision, and exit 1 if the run is invalid or tsox has false positives
./scripts/harness.sh --ref <commit> --strict

# Every option
./scripts/harness.sh --help
```

Use a release build of tsox for speed and memory. The report marks a debug build as not meaningful, and marks the whole run invalid when the compilers cannot run or the project's dependencies are missing. Linux and macOS, 64-bit.

### Running it in CI

The harness is **not** part of the `ci.sh` quality gate and never runs automatically on push or pull request — it hits the network, depends on an external fixture repository, and is expected to report diagnostics that disagree with `tsc`/`tsgo` given the current state of the project (see [Category 2/3 of the roadmap](purpose-and-overdesign.md)), so it must not block merges.

It can be run on demand from GitHub Actions instead: go to **Actions → Compatibility Harness → Run workflow**. Results are uploaded as a downloadable artifact (`harness-results`), not committed to the repository.

### Fixture-level three-way report (on demand)

`scripts/harness.sh` above benchmarks against one real project (Zustand, by default). `scripts/ts-diag-tool/compare3.js` applies the same three-way idea (`tsc`, `tsgo`, `tsox`) to `tests/fixtures/` instead — per-fixture diagnostics, timing, and memory, plus a browsable HTML report. Like the harness above, this is on-demand tooling, not part of the fast, CI-gated loop described under "Fixture-level diagnostic comparison" below.

```bash
# tsgo needs its own scratch install -- see "Fixture-level diagnostic
# comparison" below: the plain `typescript` package now resolves to v7+,
# which drops the classic JS compiler API entirely, so it can't share the
# typescript@6.0.3 install pinned in scripts/ts-diag-tool/
mkdir -p /tmp/tsgo-scratch && cd /tmp/tsgo-scratch && npm init -y && npm install typescript@7

cd /path/to/tsox
node scripts/ts-diag-tool/compare3.js tests/fixtures \
  target/release/tsox \
  /tmp/tsgo-scratch/node_modules/.bin/tsc \
  --html report.html
```

Open `report.html` in a browser: one card per compiler (error count, wall-clock time, peak memory), pairwise line-agreement counts, and an expandable per-file breakdown. `--json` gives the same data as structured output instead of the HTML/table view.

Two measurement caveats worth knowing before reading too much into the numbers:
- **tsc's memory figure is this Node process's own peak RSS**, not an isolated subprocess measurement like the other two get — there's no in-process way to isolate just the compiler API's own memory use, so this number includes the comparison tool's own overhead (V8, `cli-table3`, etc.).
- **tsgo runs one subprocess per file** (same reason `check-fixtures.js` gives `tsc` one `ts.Program` per file: fixtures across the suite commonly reuse top-level names, and non-module `.ts` files share a global scope, so batching them would produce spurious duplicate-identifier errors unrelated to real type checking). Its reported time is the *sum* across all those subprocess invocations, and its memory is the *max* peak RSS seen across them — not a single steady-state number the way a one-shot `tsc --project` run would give you.

Memory figures need [GNU time](https://www.gnu.org/software/time/) (`/usr/bin/time -v`) on your `PATH`; without it, `compare3.js` still runs and reports timing, just without the memory columns.

## Fixture-level diagnostic comparison (`scripts/ts-diag-tool/`)

Where the harness above answers "is tsox roughly as fast/compatible as tsc on a real project", this fast, CI-gated tool answers a narrower, more precise question asked continuously: "for each individual fixture under `tests/fixtures/`, which exact diagnostic (code, line, message) does tsox produce versus tsc, and where do they disagree?"

It uses the real TypeScript compiler API (not text-parsing) for `tsc`, so diagnostics are read as structured `ts.Diagnostic` objects — no regex against pretty-printed CLI output, which broke down on type names containing parens or colons in an earlier version of this tooling.

### Setup

```bash
cd scripts/ts-diag-tool
npm install
```

This pins `typescript@6.0.3` deliberately: the plain `typescript` package now resolves to v7+, which ships the native/Go-ported compiler (`tsgo`) instead of the classic JS one, and that package no longer exposes `ts.createProgram`/`ts.Diagnostics` at all.

### Running it locally

```bash
./scripts/compare-local.sh                              # every fixture under tests/fixtures
./scripts/compare-local.sh tests/fixtures/generics-tier1 # one folder
./scripts/compare-local.sh path/to/one_file.ts           # one file
./scripts/compare-local.sh tests/fixtures --only-differ  # hide fixtures where both sides fully agree
./scripts/compare-local.sh tests/fixtures --differ       # list only the lines where tsox and tsc disagree (gap / false positive / mismatch)
./scripts/compare-local.sh tests/fixtures --json         # structured output, e.g. for scripting
```

Prints one table per fixture folder: file, line, tsox's diagnostic, tsc's diagnostic, and a per-line/per-file verdict (`match`, `tsc extra (gap)`, `tsox extra (fp)`). A **gap** (tsc catches something tsox doesn't yet) is expected on a checker still being built out feature by feature. A **false positive** (tsox rejects code tsc accepts) is the more damaging kind, since it breaks valid code rather than under-checking it.

Builds `target/release/tsox` automatically if it isn't already there, and `npm install`s on first run if needed.

### Running it in CI

Unlike the harness above, this one **is** wired into CI: the `compare-tsc` job in `ci.yml` runs it against the full `tests/fixtures/` suite on the staging branch of every merge into `main`, and fails the merge only if the false-positive count exceeds what `scripts/ts-diag-tool/baseline.json` currently allows. Gaps are never gated on — they're expected while the checker is still being built out feature by feature; only false positives (which break valid code, rather than just under-checking it) block a merge.

The much larger comparison against TypeScript's own test suite is the [corpus map](corpus.md).
