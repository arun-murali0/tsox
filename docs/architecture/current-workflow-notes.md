> Snapshot of the code before the split of `narrow.rs`, `exports.rs`, `declare.rs` and `calls.rs`; file names and line counts below are from that time. See `new-architecture-progress.md` for the current layout.

# Current workflow (as read on branch new-architecture, a05b7c7c)

Working notes for the architecture refactor. Local only.

## Crate stack (strictly downward)
base < types < checker < project < db < tsox (facade) < cli | lsp | wasm | xtask
Enforced by cargo xtask layering.

## One file: `bridge::check_file` (crates/tsox-checker/src/bridge/mod.rs)
1. arena.clear(); cancel check
2. parse_with_syntax_errors (parse.rs): Oxc parse; on give-up, syntax_repair patches the text
   (OffsetMap maps offsets back); bracket-depth guard first
3. analyze: SemanticBuilder with cfg + nodes
4. CheckContext (context.rs): ONE big mutable struct, ~60 fields (arena, namespace, symbols,
   narrow overlay, flow facts, return bookkeeping, class state, probe, imports...)
5. declare_phase: imports::bind_imports, declare::declare_top_level
6. bodies_phase (skipped for .d.ts when asked and nothing needs it):
   unreachable_code::analyze_flow (reachability.rs) -> FlowShadow (flow_shadow.rs) ->
   statements::check_top_level -> shadow.report -> 8 whole-file passes called by hand
7. exports::collect_exports (needs ModuleKey) -> ExportTable (portable decls)
8. publish: drains namespace issue lists into diagnostics; sort by (start,len,code), dedup
9. Worker (unit.rs) wraps this: catch_unwind, arena replaced on panic, UnitOutput (arena-free)

## Walkers
- statements/mod.rs `check_statement`: big match; control_flow.rs does if/loops/switch/try
  with the forward NarrowState overlay (join_states, JumpFrame for break/continue)
- expressions/mod.rs `infer_expression_type`: big match; passes info via ctx fields
  (contextual, call_return_context, operand_spans, const_context ...) = hidden parameters
- narrow.rs (2306 lines): NarrowState (symbol + property-path overlays), narrow_condition
  tries one shape after another (first match wins), plus switch, predicates, instanceof, in
- Unsupported kinds emit a warning and the error type (quiet-beats-wrong)

## Flow: three mechanisms
1. reachability.rs: tsc-binder replay over AST (dead code, TS2454, return paths)
2. forward overlay: narrow.rs + statements/control_flow.rs
3. flow.rs graph + flow_shadow.rs backward resolver (oxc_cfg + AST pass), compares to overlay

## Types (tsox-types)
- TypeArena: interned Type enum (31 variants), TypeId(u32), Ref/Named/App slots for recursion
- namespace.rs: flat name -> declaration map with scope prefixes, generics, issue lists
- subtyping.rs + semantic/queries (RelationCache) + semantic/solver + generics (3.4k lines)
- portable.rs: arena-free declarations (export_decl / Importer) with DeclKey/SigHash (base/keys)
- Known: tsox-types depends on oxc_ast/oxc_semantic; thread-local state exists
  (stack_guard, relation overflow, pair memo)

## Project (tsox-project/module_resolution)
- graph (SCCs, edges), resolver (oxc_resolver), config (tsconfig), discovery (scan imports)
- check.rs `run_project`: providers (ambient decl files) first -> Globals -> pass 1 over
  SCC units via schedule::run_units (rayon ready-queue, deterministic) -> optional pass 2 when
  globals/UMD/augmentations exist (only affected files) -> cross-file return cycles (TS7023)
- cycles: members re-checked until exports stop changing (cap grows with size)
- ImportsOf implements ImportSource over Tables (names, complete flag, decls by DeclKey)

## Query layer (tsox-db, feature incremental)
- Salsa: SourceFile input, module_header, scc_plan, check_scc (lru 4096), decl_shape,
  export_names/lookup/prefix/complete, check_provider, file_report
- Scheduler still owns order; check_scc reads a Bundle (verified generation, Weak outputs)
  untracked; correctness relies on tests/incremental_project.rs equality with cold runs
- scc_run:<id> notes are the work accounting (step 0 of the proposal)

## Facts that matter for the proposal
- Proposal's claims checked so far: sizes, three flow mechanisms, inferred-return placeholder
  (context.rs), one DiagnosticMessage per call site, hand-listed passes in bodies_phase,
  std::fs in db/base/project all hold in the code I read.
- check-layering was a Python script; it is now `cargo xtask layering` (proposal A1 done).
- Diagnostics: ctx.error(message, span) pushes directly; ordering fix-ups happen twice
  (publish sort, then check_file sort+dedup).
- Workspace builds clean with rustc 1.100 (cargo check --workspace --all-features, ~67 s).

## Not yet read in detail
tsox-lsp, tsox-cli, tsox-wasm, xtask, subtyping/generics internals, class and call checking
bodies, exports.rs collector internals, config.rs, graph.rs, tests/.
