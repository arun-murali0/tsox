mod context;
mod declare;
mod exports;
mod expressions;
mod imports;
mod narrow;
mod parse;
mod statements;
mod unreachable_code;
mod unresolved_names;

use oxc_allocator::Allocator;
use oxc_ast::ast::Program;
use oxc_semantic::{Scoping, Semantic};

use crate::arena::{TypeArena, TypeArenaStats};
use crate::diagnostics::Diagnostic;
use crate::error::CheckerError;
use crate::keys::ModuleKey;
use crate::namespace::NamespaceStats;
use crate::portable::ImportSource;
use crate::semantic::queries::QueryStats;
use crate::types::FileId;
use crate::unit::CancelToken;
use context::CheckContext;
pub(crate) use exports::ExportTable;
pub use exports::IncompleteReason;

pub use parse::parse;

pub fn parse_and_bind_only(source: &str, file_name: &str) -> Result<(), CheckerError> {
    let allocator = Allocator::default();
    let program = parse(&allocator, source, file_name)?;
    let _semantic = parse::analyze(&program);
    Ok(())
}

/// Sizes and cache counters from one check, for benchmarks and regression reports.
pub struct CheckMetrics {
    pub arena: TypeArenaStats,
    pub queries: QueryStats,
    pub namespace: NamespaceStats,
}

// The whole checking pipeline for one file, in the phases LLD 4.3 gives a unit: declare
// resolves every top-level signature first, so a function can call another declared
// later in the same file, then bodies walks statement and expression bodies against
// those already-resolved signatures, then publish turns what the passes collected into
// the ordered diagnostic list. The shapes phase has no pass of its own yet, because
// declare resolves signatures eagerly; it splits out when expansion becomes lazy.
//
// The arena is borrowed, not created here, so a CheckSession can keep one allocation
// across successive versions of a file. It is cleared first, so nothing from the
// previous check (types, names, intern entries) is visible to this one, and handed
// back at the end with the retained capacity.
pub fn check_program_with_state(
    source: &str,
    file_name: &str,
    file_id: FileId,
    arena: &mut TypeArena,
) -> Result<(Vec<Diagnostic>, CheckMetrics), CheckerError> {
    match check_file(source, file_name, file_id, arena, FileEnv::default())? {
        Some((diagnostics, metrics, _exports)) => Ok((diagnostics, metrics)),
        None => unreachable!("a check without a cancel token is never cancelled"),
    }
}

// What a check reads besides the source: all optional, and `Default` is a one-file check with no
// project around it, no cancel token and the checker's default options.
#[derive(Default)]
pub(crate) struct FileEnv<'a> {
    pub cancel: Option<&'a CancelToken>,
    pub module: Option<ModuleKey>,
    pub imports: Option<&'a dyn ImportSource>,
    pub options: crate::check_options::CheckOptions,
}

pub(crate) type Checked = (Vec<Diagnostic>, CheckMetrics, ExportTable);

// The same check under an optional cancel token. A cancelled check returns None: it has
// no diagnostics to report, because the ones found before the stop are a prefix of the
// file's and would pass for the whole list. The arena goes back to the caller either
// way.
pub(crate) fn check_file(
    source: &str,
    file_name: &str,
    file_id: FileId,
    arena: &mut TypeArena,
    env: FileEnv<'_>,
) -> Result<Option<Checked>, CheckerError> {
    let FileEnv {
        cancel,
        module,
        imports,
        options,
    } = env;
    arena.clear();
    // A cut recorded by an earlier unit on this thread is not this unit's.
    crate::stack_guard::take_cut();
    if cancel.is_some_and(CancelToken::is_cancelled) {
        return Ok(None);
    }

    let allocator = Allocator::default();
    let program = parse(&allocator, source, file_name)?;
    // semantic is kept for the whole check on purpose: it owns the Scoping borrowed
    // here, and it is where the control flow graph lives once flow analysis reads
    // it. Keeping only the Scoping would drop the graph along with the rest.
    let semantic = parse::analyze(&program);

    let reusable_arena = std::mem::take(arena);
    let mut ctx = CheckContext::with_arena_and_file_id(file_name, file_id, reusable_arena);
    ctx.cancel = cancel.cloned();
    ctx.imports = imports;
    ctx.options = options;

    declare_phase(&program, semantic.scoping(), &mut ctx);
    bodies_phase(&program, &semantic, &mut ctx);
    // The guard in the namespace has no diagnostics of its own to write to, so it left a
    // flag. The position is the start of the file, since a cut chain has no one place.
    if ctx.namespace.take_depth_cut() | crate::stack_guard::take_cut() {
        ctx.nesting_too_deep(oxc_span::Span::new(0, 0));
    }
    if ctx.cancelled {
        *arena = ctx.arena;
        return Ok(None);
    }
    // Read the exports out before publish and before the arena goes back: they are
    // copied out of it, and it is cleared as soon as the unit ends. A check with no
    // module key has no identity to give a declaration, so it exports nothing.
    let mut exports = match module {
        Some(module) => exports::collect_exports(&program, semantic.scoping(), &mut ctx, module),
        None => ExportTable::default(),
    };
    // Exported functions whose return type is inferred from imported functions: the project
    // looks for a circle among them, which no one file can see. A function that is not
    // exported cannot be the way back into this file, so it is not a node of such a circle.
    exports.return_reads = ctx
        .return_reads
        .cross_file()
        .into_iter()
        .filter_map(|(_, name, span, callees, reported_locally)| {
            let owner = exports
                .decls
                .iter()
                .find(|decl| decl.kind == crate::keys::DeclKind::Function && *decl.name == *name)?
                .key;
            Some(crate::unit::ReturnRead {
                owner,
                name,
                start: span.start,
                end: span.end,
                callees,
                reported_locally,
            })
        })
        .collect();
    let (diagnostics, metrics) = publish(&mut ctx);

    *arena = ctx.arena;
    Ok(Some((diagnostics, metrics, exports)))
}

fn declare_phase<'ast>(
    program: &'ast Program<'ast>,
    scoping: &Scoping,
    ctx: &mut CheckContext<'ast, '_>,
) {
    // Imports first: a local declaration, a signature or an annotation may name any of
    // them, and the declare pass resolves in source order only within itself.
    imports::bind_imports(program, scoping, ctx);
    declare::declare_top_level(program, ctx);
}

fn bodies_phase<'ast>(
    program: &'ast Program<'ast>,
    semantic: &Semantic<'ast>,
    ctx: &mut CheckContext<'ast, '_>,
) {
    let scoping = semantic.scoping();
    statements::check_top_level(program, scoping, ctx);
    if ctx.cancelled {
        return;
    }
    unresolved_names::report_unresolved_type_names(program, scoping, ctx);
    unreachable_code::check_unreachable_code(program, semantic, ctx);
}

// Turns what the passes left in the namespace into diagnostics and returns the file's
// diagnostics in order, with the metrics of the check that made them.
fn publish(ctx: &mut CheckContext<'_, '_>) -> (Vec<Diagnostic>, CheckMetrics) {
    for span in ctx.namespace.take_too_complex() {
        ctx.error(
            crate::diagnostic_messages::messages::expression_too_complex(),
            span,
        );
    }

    for (name, span) in ctx.namespace.take_implicit_any_params() {
        ctx.error(
            crate::diagnostic_messages::messages::parameter_implicitly_any(&name),
            span,
        );
    }

    for (name, span) in ctx.namespace.take_unresolved_constraints() {
        ctx.warning(
            crate::diagnostic_messages::messages::unresolvable_type_parameter_constraint(&name),
            span,
        );
    }

    for issue in ctx.namespace.take_type_argument_issues() {
        let name = issue.name.as_str();
        if issue.expected == 0 {
            ctx.error(
                crate::diagnostic_messages::messages::type_is_not_generic(name),
                issue.span,
            );
        } else {
            // Includes a bare `Box` for `Box<T>` (given == 0), which tsc also
            // reports as a missing type argument.
            // tsc prints the generic with its parameter list: `Box<T>`, `Pair<A, B>`.
            let display_name = match ctx.namespace.declared_type_param_decl(name) {
                Some(decl) => {
                    let params: Vec<&str> =
                        decl.params.iter().map(|p| p.name.name.as_str()).collect();
                    format!("{name}<{}>", params.join(", "))
                }
                None => match ctx.namespace.imported_generic(name) {
                    Some(imported) => {
                        let params: Vec<&str> =
                            imported.params.iter().map(|p| p.name.as_str()).collect();
                        format!("{name}<{}>", params.join(", "))
                    }
                    None => name.to_string(),
                },
            };
            ctx.error(
                crate::diagnostic_messages::messages::type_argument_count_mismatch(
                    &display_name,
                    issue.required,
                    issue.expected,
                ),
                issue.span,
            );
        }
    }

    for (_, actual, constraint, span) in ctx.namespace.take_constraint_violations() {
        ctx.error(
            crate::diagnostic_messages::messages::type_argument_constraint_violation(
                &ctx.arena, actual, constraint,
            ),
            span,
        );
    }

    let metrics = CheckMetrics {
        arena: ctx.arena.stats(),
        queries: ctx.relation_cache.stats(),
        namespace: ctx.namespace.stats(),
    };
    tracing::info!(
        diagnostic_count = ctx.diagnostics.len(),
        arena_types = metrics.arena.type_count,
        relation_hits = metrics.queries.relation_hits,
        relation_misses = metrics.queries.relation_misses,
        generic_instantiations = metrics.namespace.instantiations,
        "check complete"
    );

    // tsc reports in source order. Checking is not in source order (declarations are
    // resolved ahead of bodies, and a call reports its own error before its arguments'),
    // so the list is put in order here: by start, then code, then message (LLD 8.1), so
    // two diagnostics at one position come out in the same order however they were found.
    let mut diagnostics = std::mem::take(&mut ctx.diagnostics);
    diagnostics.sort_by(|a, b| {
        (a.start, a.code.as_str(), &a.message).cmp(&(b.start, b.code.as_str(), &b.message))
    });
    (diagnostics, metrics)
}

// The one-shot entry point, for callers that check a file once and do not need to
// keep state: it owns a throwaway arena, so no FileId or arena handling leaks into
// their code.
#[tracing::instrument(skip_all, fields(file_name))]
pub fn check_program(source: &str, file_name: &str) -> Result<Vec<Diagnostic>, CheckerError> {
    let mut arena = TypeArena::new();
    Ok(check_program_with_state(source, file_name, FileId::ROOT, &mut arena)?.0)
}
