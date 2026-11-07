use oxc_ast::ast::Program;
use oxc_semantic::Scoping;

use super::context::{CheckContext, FunctionReturn};
use crate::types::Type;

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
    // Always: the return-path report needs the facts whatever the options, since a function whose
    // declared type is not `void` and that can run off its end is an error under every setting.
    if is_declaration_file(ctx.file_name) {
        return;
    }
    ctx.flow = crate::reachability::analyze(program, scoping);
}

pub fn check_unreachable_code(ctx: &mut CheckContext<'_, '_>) {
    let unreachable = !ctx.options.allow_unreachable_code;
    let fallthrough = ctx.options.no_fallthrough_cases_in_switch;
    let unassigned = ctx.options.strict_null_checks;
    report_function_returns(ctx);
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

// A function whose end can be reached, and what it says about its return. The facts come from the
// flow (can the end be reached, can a `return` be reached) and the types from the checker (what the
// function declared, what its body returned), and tsc's rules pair them in this order:
//
//   * a declared type of `void`, `any` or `undefined`, or a union with `void`, is never an error;
//   * `never` cannot have a reachable end (TS2534);
//   * a declared type and no reachable `return` at all: it must return a value (TS2355);
//   * under strict null checks, a declared type that `undefined` is not assignable to: the
//     function lacks an ending return (TS2366);
//   * otherwise, with `noImplicitReturns`, not all code paths return (TS7030). Without a
//     declared type this needs the inferred type, which is `void`, `undefined` or `any` unless the
//     body returns some other value, and a function that never returns a value is left alone.
//
// A bare `return;` is checked on its own under `noImplicitReturns`, but only without strict null
// checks: with them tsc checks it as returning `undefined` and reports that instead. It is an
// error where the return type is not `void`, `undefined` or `any`.
//
// An annotated generator is not covered, and neither is an annotated async function until
// `Promise<T>` is modelled well enough to read the `T` back. A function whose annotation the
// checker could not resolve is left alone rather than guessed at.
fn report_function_returns(ctx: &mut CheckContext<'_, '_>) {
    let implicit_returns = ctx.options.no_implicit_returns;
    let strict = ctx.options.strict_null_checks;
    for fact in std::mem::take(&mut ctx.flow.functions) {
        // What a generator's annotation means (the type its `return` produces) is not modelled.
        if fact.is_generator && fact.has_annotation {
            continue;
        }
        // A function the checker never walked (an immediately invoked one it does not check yet)
        // has no entry; without an annotation that is still answerable from the function itself.
        let info = match ctx.function_returns.get(&fact.key).cloned() {
            Some(info) => info,
            None if !fact.has_annotation => FunctionReturn {
                declared: None,
                returned: None,
            },
            None => continue,
        };
        // What the annotation comes to. From the checker's type when it has one; for an async
        // function, whose `Promise<T>` it cannot read, from the syntax of `T`. An annotation that
        // neither can read leaves the function alone.
        let declared = if !fact.has_annotation {
            None
        } else {
            match (info.declared, fact.async_inner) {
                (Some(ty), _) => Some(Declared::of_type(&ctx.arena, ty)),
                (None, Some(inner)) => Some(Declared::of_inner(inner)),
                (None, None) => continue,
            }
        };
        // Does the inferred type of an unannotated function come to `void`, `undefined` or `any`?
        // From the types the checker collected when it has them, otherwise from the return
        // statements themselves: none that return a value, or only `undefined`. A value of type
        // `any` is only seen in the first.
        let inferred_is_empty = match &info.returned {
            Some(returned) => {
                returned.is_empty()
                    || returned
                        .iter()
                        .any(|&ty| matches!(ctx.arena.get(ty), Type::Any))
                    || returned
                        .iter()
                        .all(|&ty| matches!(ctx.arena.get(ty), Type::Void | Type::Undefined))
            }
            None => !fact.returns_a_value,
        };
        if implicit_returns && !strict && !fact.is_constructor && !fact.bare_returns.is_empty() {
            let nothing_to_return = match declared {
                Some(declared) => declared == Declared::NothingNeeded,
                None => inferred_is_empty,
            };
            if !nothing_to_return {
                for span in &fact.bare_returns {
                    ctx.error(
                        crate::diagnostic_messages::messages::not_all_code_paths_return(),
                        *span,
                    );
                }
            }
        }
        if !fact.end_reachable || declared == Some(Declared::NothingNeeded) {
            continue;
        }
        let message = match declared {
            Some(Declared::Never) => {
                Some(crate::diagnostic_messages::messages::never_has_reachable_end())
            }
            Some(_) if !fact.has_explicit_return => {
                Some(crate::diagnostic_messages::messages::must_return_a_value())
            }
            Some(Declared::UndefinedNotAllowed) if strict => {
                Some(crate::diagnostic_messages::messages::lacks_ending_return())
            }
            declared if implicit_returns => {
                let skip = declared.is_none() && (!fact.has_explicit_return || inferred_is_empty);
                (!skip).then(crate::diagnostic_messages::messages::not_all_code_paths_return)
            }
            _ => None,
        };
        if let Some(message) = message {
            ctx.error(message, fact.error_span);
        }
    }
}

// What a declared return type means for a function that can run off its end.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Declared {
    // `void`, `any`, `undefined`, or a union with `void`: nothing needs to be returned.
    NothingNeeded,
    Never,
    // `undefined` is a value of the type (`unknown`, `number | undefined`).
    UndefinedAllowed,
    UndefinedNotAllowed,
}

impl Declared {
    fn of_type(arena: &crate::arena::TypeArena, ty: crate::arena::TypeId) -> Declared {
        if type_is_void_undefined_or_any(arena, ty) {
            Declared::NothingNeeded
        } else if matches!(arena.get(ty), Type::Never) {
            Declared::Never
        } else if crate::subtyping::is_subtype(arena, arena.undefined(), ty) {
            Declared::UndefinedAllowed
        } else {
            Declared::UndefinedNotAllowed
        }
    }

    fn of_inner(inner: crate::reachability::PromiseInner) -> Declared {
        use crate::reachability::PromiseInner;
        match inner {
            PromiseInner::VoidUndefinedOrAny => Declared::NothingNeeded,
            PromiseInner::Never => Declared::Never,
            PromiseInner::AcceptsUndefined => Declared::UndefinedAllowed,
            PromiseInner::RejectsUndefined => Declared::UndefinedNotAllowed,
        }
    }
}

// tsc's `maybeTypeOfKind(type, Void)` or `any` or `undefined`: the declared types for which
// returning nothing is fine.
fn type_is_void_undefined_or_any(
    arena: &crate::arena::TypeArena,
    ty: crate::arena::TypeId,
) -> bool {
    match arena.get(ty) {
        Type::Any | Type::Void | Type::Undefined => true,
        Type::Union(members) => members
            .iter()
            .any(|&member| matches!(arena.get(member), Type::Void)),
        _ => false,
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
