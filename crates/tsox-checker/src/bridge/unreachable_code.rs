use oxc_ast::ast::Program;
use oxc_semantic::Semantic;

use super::context::CheckContext;

// Reports code that can never run (TS7027), once for each run of adjacent dead statements and
// at the first of them, which is what tsc does.
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
pub fn check_unreachable_code<'a>(
    program: &Program<'a>,
    _semantic: &Semantic<'a>,
    ctx: &mut CheckContext<'_, '_>,
) {
    let unreachable = !ctx.options.allow_unreachable_code;
    let fallthrough = ctx.options.no_fallthrough_cases_in_switch;
    if !(unreachable || fallthrough) || is_declaration_file(ctx.file_name) {
        return;
    }
    let found = crate::reachability::analyze(program);
    if unreachable {
        for span in found.reports {
            ctx.error(
                crate::diagnostic_messages::messages::unreachable_code(),
                span,
            );
        }
    }
    // TS7029 is the same question asked at the end of a `case`: can the flow still be here. It
    // rides along with TS7027 because both read the one reachability answer.
    if fallthrough {
        for span in found.fallthroughs {
            ctx.error(
                crate::diagnostic_messages::messages::fallthrough_case_in_switch(),
                span,
            );
        }
    }
}

fn is_declaration_file(file_name: &str) -> bool {
    file_name.ends_with(".d.ts") || file_name.ends_with(".d.mts") || file_name.ends_with(".d.cts")
}
