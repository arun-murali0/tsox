# The language server (`tsox-lsp`)

`tsox-lsp` serves the Language Server Protocol over stdio, on tower (`tower-lsp-server`). It
runs tsox's own parser, binder and checker in-process: no Node, no `tsc`. The `tsc`-style
codes on diagnostics (`TS2322`) are only labels the checker attaches to what it found itself.

## Status

| Phase | What | State |
| --- | --- | --- |
| 1 | UTF-16 positions, incremental text edits, diagnostic and URI conversion | done |
| 2 | Worker thread that owns the project; cancellation; diff-only reporting | done |
| 3 | Tower server: lifecycle, open/change/close, publishDiagnostics, watched files | done |
| 4a | Document symbols (outline) | done |
| 4b | Go to definition, find references | done |
| 4c | Hover (declaration headers, JSDoc, checker types) | done |
| 5 | Completion (scope, namespace and enum members, module specifiers), signature help | done, except members of ordinary values |
| 6 | Rename, document highlight, workspace symbols, folding ranges, selection ranges | done |
| 6b | Code actions, semantic tokens, inlay hints | not started |
| 7 | Hardening, performance, packaging | not started |

Working feature list today: `textDocument/publishDiagnostics`, `documentSymbol` (nested and
flat), `definition`, `references`, `hover`, `completion`, `signatureHelp`, `rename` and
`prepareRename`, `documentHighlight`, `foldingRange`, `selectionRange`, `workspace/symbol`, and
`workspace/didChangeWatchedFiles` handling.

## How it is put together

```
editor ── stdio ── Backend (server.rs) ──┬── documents (open text, under one lock)
                                         ├── outbox ── one task ── client   (all output)
                                         └── Worker (worker.rs)  ── owns IncrementalProject
                                                  ▲  commands (cancel the running check)
                                                  │  queries  (never cancel; answered between checks)
```

* **`text.rs`** — `LineMap` converts between byte offsets and LSP positions (zero-based
  lines, UTF-16 columns; `\n`, `\r\n`, `\r`). `Document::apply_changes` applies `didChange`
  edits in order.
* **`worker.rs`** — `IncrementalProject` takes `&mut self` and holds a Salsa database that is
  not `Sync`, so one thread owns it. A command (buffer change, close, files changed) cancels
  the check in progress through the checker's `CancelToken`; the worker then takes everything
  queued, applies it in order, and checks once. The queue and the token share one lock, so no
  command waits behind a check that started after it arrived. Only the diagnostics of open
  documents that differ from what was last sent are reported, tagged with the version
  checked. A panic while checking drops the project (reloaded on the next command) instead of
  killing the server. Queries (`project_files`, `type_at`) never cancel a check; if the check
  they follow is overtaken they are put back and answered after the next one.
* **Revision safety.** A type query carries the version of the document its offset is into; the
  worker answers it only if it holds that version, so hover is never a type from newer text at
  an older offset. Definition, references and the rest read one snapshot per request.
* **`server.rs`** — the tower service. Notification handlers (`didOpen`, `didChange`,
  `didClose`) do all their work before their first await, so even though the transport may
  run handlers concurrently they are applied in arrival order. Nothing is sent from a handler:
  every outgoing message goes through one queue, filled under the documents lock, and one
  task sends it, so a late diagnostic can never overtake the clearing of a closed document.
  A report for a document that has since changed is dropped and the worker is told to
  `Forget` it, so the next check reports it afresh.
* **`analysis.rs`** — every feature that reads syntax goes through `with_program`. The parser
  recovers from many errors but a fatal one (an unclosed brace, i.e. a half-typed
  declaration) leaves it with no tree. Then the text is cut at the start of the line holding
  the error and read again, until something parses, so a file mid-edit still answers for what
  comes before the error. Offsets stay valid because the cut is a prefix.
* **`symbols.rs`** — the outline, from syntax alone.
* **`navigate.rs`** — definition and references from syntax and binder data, with a `Host`
  trait (read a file, resolve a specifier) so it runs against the server or a fake.
  Follows imports, `export … from`, `export *`, renamed exports, default exports and
  namespace members across files. References scan the project's files (and open documents)
  for importers, to a fixed point over re-export chains, and stop between files when the
  request is cancelled.
* **`hover.rs`** — a one-line header from the declaration's syntax, the JSDoc above it, and
  the checker's type where it has one (see below).
* **`convert.rs`, `paths.rs`** — diagnostics to LSP form; URIs to the canonical paths the
  project graph keys files by.

### The type probe (core change)

Types live in a file's arena and are gone when its check ends, so they cannot be asked of a
finished check. `IncrementalProject::type_at(path, offset)` (in `tsox-db`, via
`probe_unit_isolated` in `tsox-project` and `ProbeRequest` in `tsox-checker`) re-checks that
one file against the cached exports of its dependencies with a request riding along: each
expression that holds the offset offers its type, the innermost is kept as text, and a name
at its declaration is read from the symbol table. Callees (calls, `new`, method calls) offer
their own type explicitly because they are not typed through the expression typer. With no
request open none of this runs; normal checks are unchanged, and the incremental suite
(including the randomized "always equals a cold check" tests) passes with the change. The
checker's internal `error` type is never reported.

## Running it

```
cargo build -p tsox-lsp --release      # target/release/tsox-lsp
tsox-lsp --stdio                       # --stdio is accepted; it is the only transport
TSOX_LSP_LOG=debug tsox-lsp            # logs go to stderr
```

The project is the workspace folder's `tsconfig.json`, else the nearest one above the first
open file, else the open files themselves.

## Remaining work

### Completion and signature help: what is left
Done: names in scope (innermost first, with kinds and signatures), keywords, exports of a
namespace import (following `export *`), enum members, module specifiers (relative files and
folders, `node_modules` packages), and signature help for a call that is still being typed
(the checker's type of the callee when it has one, else the declaration; active parameter by
commas at the call's own level; rest parameters; JSDoc). It works on half-typed code through a
small scanner and a closed-off copy of the text up to the cursor.
* **Members of ordinary values** (`object.|`, `this.|`, a call's result): needs a checker query
  that lists the members of the type at an offset, the way `type_at` reports its type. Until
  then these give nothing.
* **Auto-import** (offer a name that is not imported and add the import as an edit);
  named-import completion inside `import { | } from './x'` from the target's exports.
* **`completionItem/resolve`** for documentation, so the first response stays small.
* `tsconfig` `paths` aliases and `exports` maps in module specifier completion; text edits for
  specifier items so the replaced range is the whole typed path.
* Library globals (`console`, `Math`, …) are not offered, because the project's global
  declarations are not exposed to the server.
* **Signature help**: overloads (only one signature is shown), constructors (`new Foo(|)`)
  when the checker gives no type, and `retriggerCharacters`.

### Rename and structure: what is left
Done: rename across files with alias-preserving edits (`import { a }` becomes
`import { b as a }`, so a module's public names and the other file's uses are left alone),
refusing members, globals, library declarations, invalid and clashing names; document
highlight with read/write kinds; folding ranges (bodies, imports, comments, regions);
selection ranges; workspace symbols over the project's files and open documents.
* **Rename of members** (properties, methods) and of names in script files (no imports or
  exports, whose top-level names are global across files): both need symbol identity from the
  checker, which the server does not have; rename refuses them rather than guess.
* Rename of a file/module path, and import-path updates when a file moves
  (`workspace/willRenameFiles`).
* **Code actions** (quick fixes keyed by diagnostic code, organize imports, remove unused),
  **semantic tokens**, **inlay hints** (batched probe, not one re-check per offset), type
  definition / implementation / declaration requests.

### Diagnostics
* Related information and tags (unnecessary / deprecated), where the checker has them.
* Pull diagnostics (`textDocument/diagnostic`, 3.17) and workspace diagnostics; an option to
  publish for all project files, not only open ones.
* Documents that are not files on disk (untitled, `untitled:`/`vscode-notebook-cell:`)
  get no project and no diagnostics today.
* **Multiple projects**: several workspace folders and several `tsconfig.json` (project
  references); the worker holds one project. Files outside any `include` get an inferred
  project like tsserver's.
* `workspace/configuration` and `didChangeConfiguration`: what to report, tsc-style codes on
  or off, max file size, features on or off.
* Work-done progress while a large project loads; position-encoding negotiation (UTF-8 and
  UTF-32 as well as UTF-16); `didSave` handling; `shutdown` behaviour for requests that
  arrive after it.

### Navigation and hover gaps
* **Member definition and references** (`object.member`, `this.member`): needs a checker
  query that maps a member access to its declaration span (probe variant returning a span,
  not text). Until then these have no definition and hover shows only the access's type.
* `export =`, `module.exports`/CommonJS in JS files, `import x = require()`.
* References scan every project file that mentions the name; replace with the module graph's
  dependents to avoid reading files that cannot import it. A references request also waits
  for a check in progress (it asks the worker for the file list); keep a cached file list
  that updates on graph changes.
* Hover: `{@link}` in JSDoc, hover on keywords/`this`/literal types, JSX tag names,
  per-(file version, offset) probe cache, a cancelable probe (a hover waits behind a running
  check today).
* Class instances display structurally in hover (`{ get(): number; item: number; }`) because
  the checker's type display does; showing `Box<number>` is a change in `type_display.rs`.

### Performance and robustness
* Cache parse + binder data per document version (every request re-parses today; fine at
  oxc's speed, worth measuring on very large files).
* Bound request work: a deadline on references; propagate tower request cancellation into the
  blocking task (references already stops between files; definition and hover do not).
* Fuzz/property tests for `LineMap` and `Document::apply_changes` (UTF-16 surrogates, mixed
  line endings, out-of-range ranges).
* A conformance harness that runs a corpus through both the server and the CLI and compares
  diagnostics.
* End-to-end tests against a real editor client (VS Code extension, Neovim `vim.lsp`), and a
  minimal VS Code extension in `editors/vscode`.

### Checker gaps that surfaced
These are core limitations, not LSP bugs, but they limit what hover can show:
* The checker holds its internal `error` type, silently, for some library members:
  `Date.now()`, `Promise.resolve(…)`, `JSON.stringify(…)`; hover shows nothing for them.
  Array callback methods give `any[]` (`[1, 2].map(x => x * 2)`).
* In a circular import, a file probed with `type_at` can see `any`/missing types for names
  from its cycle mates (the probe checks the whole component, but members' exports among
  themselves are only as good as the checker's cycle settling).
* A file with a fatal parse error is not checked at all, so the probe returns nothing for it
  (outline, definition and references still work through `analysis.rs` recovery).

### Packaging and project hygiene
* CI job: build, `cargo test -p tsox-lsp`, clippy `-D warnings`, fmt check.
* Release binary and install instructions; a `tsox lsp` subcommand in `tsox-cli` that
  launches the server.
* Pin and review `tower-lsp-server` upgrades (it is a young fork of the unmaintained
  `tower-lsp`).
* Keep `tests/type_at.rs` and the probe in step with checker changes (it asserts real
  inferred types).
