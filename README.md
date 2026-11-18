<div align="center">

# tsox

**A TypeScript type checker written in Rust, built on [Oxc](https://oxc.rs/).**
One engine as a command line tool, a language server, and WebAssembly.

[![CI](https://github.com/Arun-murali0/tsox/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/Arun-murali0/tsox/actions/workflows/ci.yml)
[![Docs](https://github.com/Arun-murali0/tsox/actions/workflows/docs.yml/badge.svg)](https://arun-murali0.github.io/tsox/)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#license)
[![Rust](https://img.shields.io/badge/rust-1.98%2B-orange)](Cargo.toml)
[![Status](https://img.shields.io/badge/status-experimental-yellow)](#status)

[Documentation](https://arun-murali0.github.io/tsox/) ·
[Install](https://arun-murali0.github.io/tsox/guide/install) ·
[How it works](https://arun-murali0.github.io/tsox/architecture) ·
[Contributing](https://arun-murali0.github.io/tsox/contributing) ·
[Report a problem](https://github.com/Arun-murali0/tsox/issues)

`typescript` · `type-checker` · `rust` · `oxc` · `language-server` · `wasm` · `compiler`

</div>

## Status

tsox is **experimental**. Do not use it as your only TypeScript check. It reports what it finds
itself and can differ from `tsc`: it misses some errors (gaps) and, less often, reports an error
`tsc` does not (a false positive).

How close it is to `tsc` is measured, not claimed. Every change is run over TypeScript's own test
suite (the single-file cases of release **v6.0.3**, 10,083 files) and compared with a recorded map
of what `tsc` says:

<!-- corpus-status:start -->
| | |
| --- | --- |
| Files where tsox and tsc agree exactly | **5,100 of 10,083** (50.6%) |
| Lines with a diagnostic from either side | 37,851, of which 17,677 agree |
| Gaps (tsc reports, tsox misses) | 13,725 lines; most often TS2322 (1334), TS7006 (716), TS2339 (640), TS1005 (557), TS2304 (501) |
| False positives (tsox reports, tsc does not) | 2,721 lines; most often TS2322 (736), TS2345 (542), TS2339 (528), TS2693 (199), TS2344 (134) |
| Corpus | TypeScript v6.0.3, recorded in `tests/corpus/typescript-6.0.3` |
<!-- corpus-status:end -->

The numbers come from [`tests/corpus`](tests/corpus) and are described in
[the corpus map](https://arun-murali0.github.io/tsox/corpus). What it catches and what it does not yet
is in [language coverage](https://arun-murali0.github.io/tsox/language-coverage) and the
[roadmap](https://arun-murali0.github.io/tsox/roadmap).

## Install

Prebuilt programs (`tsox`, `tsox-lsp`) for Linux x86_64 and Windows x86_64 are attached to each
[release](https://github.com/Arun-murali0/tsox/releases). Or build from source (Rust 1.98 or newer):

```sh
git clone https://github.com/Arun-murali0/tsox
cd tsox
cargo install --path crates/tsox-cli     # the `tsox` command
cargo install --path crates/tsox-lsp     # the `tsox-lsp` language server
```

In JavaScript, the WebAssembly build: `npm install tsox`.

## Use

**Command line**

```sh
tsox --project path/to/tsconfig.json
```

It reads the config, checks the files it selects, follows imports with `paths`, `baseUrl`, `extends`
and `references`, and prints diagnostics. `--incremental` and `--watch` keep the project in memory.
`tsox --help` lists everything.

**Editors.** `tsox-lsp` provides diagnostics, hover, completion, signature help, go to definition,
references, rename, outline, folding and workspace symbols. A [VS Code extension](editors/vscode) and
a [Zed extension](editors/zed) start it for you; any editor that can launch `tsox-lsp --stdio` works.

**JavaScript and the browser**

```js
import init, { TsoxChecker } from "tsox";
await init();
const checker = new TsoxChecker();
console.log(checker.checkSource("let a: string = 1;", "a.ts"));
// [{ severity: "Error", message: "Type 'number' is not assignable to type 'string'.", ... }]
```

More in the [guide](https://arun-murali0.github.io/tsox/guide/install).

## How it is built

Oxc supplies the parser, AST and scopes. tsox is the part that makes TypeScript mean something:

```text
TypeScript source ─► Oxc (parse, scopes) ─► bridge ─► semantic core ─► diagnostics
                                                      types · relations · generics · flow
```

- a type arena with `TypeId` interning, structural assignability, generic inference and substitution,
  control-flow narrowing, classes, intersections, stable keys for types that cross files;
- a project layer: module resolution, a dependency graph, a scheduler, partial results when a file
  cannot be fully checked, and an incremental query layer that re-checks only what an edit can affect;
- rules as tables (passes, narrowers, call effects, repairs), so a new rule is a new row;
- one semantic model per file for tools that come after the checker (emit, tooling).

The crates, in dependency order: `tsox-base` · `tsox-types` · `tsox-model` · `tsox-checker` ·
`tsox-project` · `tsox-db` · `tsox` (facade) · `tsox-ir` · `tsox-cli` · `tsox-emit` · `tsox-lsp` ·
`tsox-wasm` · `tsox-xtask`. A script enforces that each depends only on those before it. The full tree
and the milestone-to-test map are in [project structure](docs/project-structure.md); the design notes are
under [docs/architecture](docs/architecture).

## Develop

```sh
bash scripts/ci.sh                                   # fmt, layering, tests: what CI runs
cargo test --workspace --all-features                # tests only

cargo xtask corpus fetch                      # TypeScript's test suite, by tag
cargo xtask corpus run && cargo xtask corpus verify    # the corpus map
```

The checks that gate a merge are described in [contributing](docs/contributing.md) and
[the corpus map](docs/corpus.md).

## Contributing

Bug reports, failing cases and pull requests are welcome. The smallest source that shows a difference
from `tsc` is the best report. Every branch must pass **All green** (fmt, layering, tests) and
**Corpus map** (nothing worse than the recorded results). `main` only receives changes that passed
them. The owner's own branches merge automatically; a pull request from anyone else is reviewed and
merged by the owner. See [CONTRIBUTING.md](CONTRIBUTING.md).

## Documentation

The site is built from [`docs/`](docs) and published at **https://arun-murali0.github.io/tsox/**
(`cd docs && npm ci && npm run dev` to read it locally).

## Security

See [SECURITY.md](SECURITY.md).

## License

Dual licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
