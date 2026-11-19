# New architecture: progress on branch `new-architecture`

Plan: docs/architecture/modular-architecture-proposal.md. Current code reading:
docs/architecture/current-workflow-notes.md.

## Done (no change to checker output, layering ok)

| Area | Change |
| --- | --- |
| Structure | `tsox-model` (semantic model tables, hash, format version), `tsox-emit` (JS emit over Oxc), `tsox-ir` (typed IR skeleton); all in `cargo xtask layering` and `workspace.md` |
| Open/closed | `bridge/passes.rs`: the seven whole-file passes are a table (`PASSES`), run in table order; a test checks unique names, docs and that the unreachable-code pass runs last |
| Dependency inversion | `tsox_base::host::Host` with `MemoryHost` (test double, WASM default) and `tsox_project::host::FsHost` (native) |
| Algorithms | `publish`: the implicit-any duplicate check scanned every diagnostic per parameter (quadratic); now one set lookup. Array literals: element-type dedupe was a list scan per element; now a set beside the ordered list |
| Structure | `bridge/narrow.rs` split into 12 feature files (state, condition, switch, forms, in_operator, instanceof, typeof_tags, literal, truthiness, predicate, destructure, equality); equality narrowing is a first-match rule table (`EQUALITY_RULES`) |
| Measurement | `tests/rerun_accounting.rs`: first check 13 units; unchanged save 0; body edit 1; script edit 1; option change 13 of 13 (whole config invalidates every unit: first query-layer fix) |
| Structure | `exports/` (items, register), `declare/` (enums, functions) and `expressions/calls/` (overloads, promise) split by feature; behavior unchanged |
| Finding | `EffectiveCompilerOptions` already holds only checking options (strict flags, target, lib, noLib, skipLibCheck), so nothing can be dropped from the unit key; a smaller re-check on option change needs the typed read log (which unit read which option) |

## Next (proposal order)

1. Flow fallback counters by reason; typed read log so an option change re-checks only the units that read it.
2. Checker writes `SemanticModel` (one writer per fact), all on `UnitOutput.model`. Done: `module_info` (sorted exports, import uses); `const_values` (enum members, keyed by `tsox_model::enum_member_key`, f64 stored as bits); `node_symbol` (references to exported declarations; ones to unexported names are not recorded); `node_type` (exported declarations only, `Unchecked` when the type holds `any`/error); `flow_facts` (reads the backward flow walk answered, keyed by variable name and read order). Emit reads the model in `tsox-emit/src/model_pass.rs`: import elision, and `const enum` inlining for enums declared in the same file (an exported one keeps its declaration; cross-file inlining is not done). Gaps: expression-node types, and reads the flow walk leaves to the overlay, are not in the model yet.
3. Host: tsconfig loading (`ProjectConfig::load_with_host`) and the graph's source reads (`ModuleGraph::with_host`) are on it. The resolver's nearest-`package.json` module type and the graph's file-exists check are on it too (`ModuleResolver::with_host`). `include` and `files` selection run on it too (`Host::read_dir`/`real_path`, `ProjectConfig::entry_files_with_host`). The type-package entry and the graph's package-identity manifest read through it as well. The bundled library files are embedded in the binary (not read from disk). The library-unit cache is behind `LibraryUnitCache`: `DiskLibraryUnitCache` is the native adapter and `HostLibraryUnitCache` keeps entries in any `Host`'s cache. The only code left on the real disk is `oxc_resolver` (it reads the disk itself), the disk cache adapter and the file-state fingerprints in `metadata.rs`, which are the native adapter's own job.
4. Registries done: `PASSES`, `NARROWERS`, `EQUALITY_RULES`, `EFFECTS`, `CALL_FORMS`, `REPAIRS` (see registries.md).
5. Typed read log and per-body memo (shadow mode first).

## Order of work (set by the maintainer)

1. Semantic checker and tsc conformance (query layer, flow, registries, `SemanticModel` written by the checker).
2. JS emit (`tsox-emit` reads the model).
3. WASM, npm package, VS Code and Zed extensions.
4. IR and lowering: deferred for at least six months; `tsox-ir` stays a scaffold and is not extended.

## Remaining-work register (`remaining-work-gap-register.md`)

| Item | State |
| --- | --- |
| W1.1 split `generics.rs` | Done: `semantic/generics/{infer,instantiate,reduce/*}.rs`, identical tests |
| W1.2 shared budget, named exhaustion | Done for cuts: `stack_guard::CutReason` (stack, instantiation depth, instantiation count, recursive application, alias body nesting), counted per reason; TSR9102 unchanged. One `Budget` object with explicit passing is still open |
| W1.3 reducer contract and registry | Done for six operators: `reduce_operator` returns `Reduced`/`Deferred`/`TooComplex`; `Record` still direct. The solver's `Resolved/Partial/Failed` outcome is open |
| W1.4 inference trace | Done: `GenericInferenceSession::with_trace` and `finish_traced` (`TraceEvent`), zero cost when off; not yet wired to a CLI flag |
| W1.5-1.7 priorities, return-type inference, higher-order | Open: needs the tsc 6.0.3 priority list and corpus runs on a tracked baseline |
| W2 options accessor and read log | Done: `CheckContext::option` records reads; `FileReport.options_read` carries them; `unit_environment` no longer holds the checking options; `check_scc` registers a tracked `option_value` per option read (`register_option_reads`). Measured: an option no unit reads re-checks 0 of 13 units (was 13); with one reader, exactly that unit reruns, and its report equals a cold check's. Results replayed from the snapshot cache claim all options (conservative) |
| W2 finding | 8 of the 19 `CheckOptions` flags are never read by the checker: `strict`, `no_implicit_any`, `no_implicit_this`, `strict_function_types`, `strict_bind_call_apply`, `strict_property_initialization`, `strict_builtin_iterator_return`, `exact_optional_property_types`. The checker behaves as if the strict family were always on |
| W2 unused-code pass, missing options | Open |
| W3, W4, W5, W6 | Open |
| W7 fuzzing, panic safety | Open (panic containment per unit already exists in `unit.rs`) |
| W8 open-source readiness | Open |
| W9 | Open |
| W10 owner map | Done: `cargo xtask owners` writes `diagnostic-owners.md` (189 codes, message function, raising areas, 9000s backlog of 14, never-raised codes: `ReadonlyToMutable`, `RedeclareExportedVariable`); `ci.sh` checks it is current. A fixture-per-code column is still open |

## Limits

Every numeric limit a walk may reach is one `Budget` (`tsox_base::stack_guard::BUDGET`): substitution depth, substitution steps per statement, recursive applications, bracket depth. A walk that reaches one records its `CutReason`.
