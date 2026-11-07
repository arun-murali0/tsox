use oxc_ast::ast::Program;
use oxc_semantic::Scoping;

use super::context::CheckContext;

// Reports code that can never run (TS7027), once for each run of adjacent dead statements and
// at the first of them, which is what tsc does, and `case` clauses that fall through (TS7029).
//
// What is dead is decided by `crate::reachability`, which replays tsc's binder over the AST. It
// used to be asked of oxc's control-flow graph with a few constant conditions patched in by
// hand; the graph is a correct graph of JavaScript but not the one tsc builds, and the patches
// could not reach the cases where a constant condition decides what follows a loop or an `if`.
// See the module comment there for the rules, and docs/architecture/flow.md for how this fits
// the rest of the flow work.
//
// Not reported yet, because they need types: code after a call to a function that returns
// `never` or asserts `false`, and code after a `switch` that exhausts its discriminant.
//
// Declaration files hold no statements that run, so they are skipped without a walk.

// Runs the flow once, before any body is checked. The identifier check reads what it found about
// unassigned variables, so this has to come first; the TS7027 and TS7029 reports below read the
// same result afterwards.
pub fn analyze_flow(program: &Program<'_>, scoping: &Scoping, ctx: &mut CheckContext<'_, '_>) {
    let wanted = !ctx.options.allow_unreachable_code
        || ctx.options.no_fallthrough_cases_in_switch
        || ctx.options.strict_null_checks;
    if !wanted || is_declaration_file(ctx.file_name) {
        return;
    }
    ctx.flow = crate::reachability::analyze(program, scoping);
}

pub fn check_unreachable_code(ctx: &mut CheckContext<'_, '_>) {
    let unreachable = !ctx.options.allow_unreachable_code;
    let fallthrough = ctx.options.no_fallthrough_cases_in_switch;
    let unassigned = ctx.options.strict_null_checks;
    if !(unreachable || fallthrough || unassigned) {
        return;
    }
    if unreachable {
        for span in std::mem::take(&mut ctx.flow.reports) {
            ctx.error(
                crate::diagnostic_messages::messages::unreachable_code(),
                span,
            );
        }
    }
    // TS7029 is the same question asked at the end of a `case`: can the flow still be here. It
    // rides along with TS7027 because both read the one reachability answer.
    if fallthrough {
        for span in std::mem::take(&mut ctx.flow.fallthroughs) {
            ctx.error(
                crate::diagnostic_messages::messages::fallthrough_case_in_switch(),
                span,
            );
        }
    }
    // TS2454. The flow pass found reads that can be reached without an assignment; whether each
    // is an error is a question about the declared type. A variable declared `number |
    // undefined`, `any`, `unknown` or `void` may be read before it is assigned, and so may one
    // with a `!` assertion, which the pass already left out. This runs here, after the bodies
    // are checked, and not where an identifier's type is inferred, so a read inside an
    // expression the checker does not model yet is still reported.
    if unassigned {
        for read in std::mem::take(&mut ctx.flow.unassigned) {
            // The checker's type when it has one; otherwise what the annotation says by its
            // syntax, and nothing when that needs a name resolved.
            let allows = match ctx.symbols.get(read.symbol) {
                Some(declared) => Some(type_allows_unassigned_read(&ctx.arena, declared)),
                None => read.allows_unassigned,
            };
            if allows == Some(false) {
                ctx.error(
                    crate::diagnostic_messages::messages::used_before_assigned(&read.name),
                    read.span,
                );
            }
        }
    }
}

// tsc's `assumeInitialized` for the declared type: `any`, `unknown` and `void` are, and so is a
// type that already contains `undefined`.
fn type_allows_unassigned_read(
    arena: &crate::arena::TypeArena,
    declared: crate::arena::TypeId,
) -> bool {
    use crate::types::Type;
    let lets_through = |id: crate::arena::TypeId| {
        matches!(
            arena.get(id),
            Type::Any | Type::Unknown | Type::Void | Type::Undefined | Type::Error
        )
    };
    match arena.get(declared) {
        Type::Union(members) => members.iter().any(|&member| lets_through(member)),
        _ => lets_through(declared),
    }
}

fn is_declaration_file(file_name: &str) -> bool {
    file_name.ends_with(".d.ts") || file_name.ends_with(".d.mts") || file_name.ends_with(".d.cts")
}
