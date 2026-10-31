use oxc_ast::ast::{Function, Statement};
use oxc_semantic::Scoping;
use oxc_span::GetSpan;

use super::super::context::CheckContext;
use super::super::declare::{declare_function, resolve_function_type};
use super::support::report_implicit_any_params;
use super::{bind_params, check_statement};

pub(super) fn check_function_declaration(
    func: &Function,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) {
    report_implicit_any_params(&func.params, ctx);

    let Some(body) = &func.body else {
        // An anonymous default function in a declaration file has no body. Its
        // explicitly written signature is still a valid export.
        if func.id.is_none()
            && let Some(signature) = resolve_function_type(func, ctx)
        {
            ctx.exported_function_types
                .insert(func.span().start, signature);
        }
        return;
    };
    let Some(name) = func.id.as_ref() else {
        check_anonymous_function_body(func, body, scoping, ctx);
        return;
    };
    let Some(symbol_id) = name.symbol_id.get() else {
        return;
    };

    // Reuses the signature the declare pass already resolved, rather than
    // re-resolving parameter and return types here, so the body is checked
    // against exactly the same types the function was declared with. A function
    // nested in another body was not part of that pass and is declared now if
    // hoist_function_declarations has not already done it.
    if ctx.symbols.get(symbol_id).is_none() {
        declare_function(func, ctx);
    }
    let Some(declared) = ctx.symbols.get(symbol_id) else {
        tracing::trace!(name = %name.name, "function signature not fully annotated, body not checked");
        return;
    };
    // A function that shares its name with a namespace is declared as the intersection of
    // its signature and the namespace's object (see declare_namespace_values), and the body
    // is checked against the signature half. Taking only a plain function type here would
    // leave the body of every merged function unchecked, without a word.
    let function_type = match ctx.arena.get(declared).clone() {
        crate::types::Type::Function(function) => function,
        crate::types::Type::Intersection(members) => {
            let signature = members
                .iter()
                .find_map(|&member| match ctx.arena.get(member) {
                    crate::types::Type::Function(function) => Some(function.clone()),
                    _ => None,
                });
            let Some(signature) = signature else { return };
            signature
        }
        _ => return,
    };

    bind_params(&func.params, &function_type.params, scoping, ctx);

    // A function body starts with no narrowing at all, not with whatever the
    // caller's scope had established: this checker does not narrow a closed-over
    // variable inside a nested function, so nothing from the enclosing scope
    // applies here, and without resetting, narrowing from one top-level function
    // would otherwise leak into the next one checked after it.
    let outer_narrow = std::mem::take(&mut ctx.narrow);

    // Pushed again here, separately from the declare pass's own push, since the
    // declare pass and this body-check pass are two independent traversals. Both
    // must resolve func's own T to the exact same GenericParameter node, which is
    // what TypeNamespace's per-parameter identity cache guarantees; without it, a
    // T[] annotation written inside the body would not match the parameter T
    // came from.
    let type_param_scope = ctx.namespace.push_type_params(&mut ctx.arena, func);

    // No return annotation means the signature carries the placeholder `any` and the
    // body's returned types are collected to replace it (see declare_function).
    let infers_return = func.return_type.is_none();
    let return_scope = ctx.enter_function_return_scope(
        Some(function_type.return_type),
        infers_return,
        func.r#async,
    );
    if infers_return {
        ctx.return_reads
            .note_owner(symbol_id, name.name.as_str(), name.span());
    }
    hoist_function_declarations(&body.statements, ctx);
    for body_stmt in &body.statements {
        check_statement(body_stmt, scoping, ctx);
    }
    let returned = ctx.leave_return_scope(return_scope);
    ctx.narrow = outer_narrow;

    if infers_return {
        // A return type that is read, directly or through other functions, by one of its
        // own return expressions has no answer. It stays `any`, and now says so.
        for (function, span) in ctx.return_reads.take_cycle(symbol_id) {
            ctx.error(
                crate::diagnostic_messages::messages::return_implicitly_any(&function),
                span,
            );
        }
        // Every `return` contributed its widened type; a body that never returns a
        // value (or only falls off the end) returns void, the way tsc infers it.
        // Re-declaring replaces the placeholder, so calls checked after this point
        // see the real return type.
        let inferred = if returned.is_empty() {
            ctx.arena.void()
        } else {
            ctx.arena.alloc_union(returned)
        };
        // An async function returns a Promise of what the body returns, and a returned
        // Promise is awaited.
        let inferred = if func.r#async {
            let awaited = ctx.awaited_type(inferred);
            ctx.arena.alloc_promise(awaited)
        } else {
            inferred
        };
        let updated = ctx
            .arena
            .alloc(crate::types::Type::Function(crate::types::FunctionType {
                params: function_type.params,
                return_type: inferred,
                type_params: function_type.type_params,
                is_untyped: function_type.is_untyped,
            }));
        ctx.symbols.declare(symbol_id, updated);
    }

    ctx.namespace.pop_type_params(type_param_scope);
}

// Checks the body of `export default function (...) { ... }`. Unlike a named
// function declaration it has no SymbolId where its inferred signature can be stored,
// so publish the completed FunctionType under the declaration's stable source offset
// for the export collector to serialize after the body pass.
fn check_anonymous_function_body(
    func: &Function,
    body: &oxc_ast::ast::FunctionBody,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) {
    let Some(signature) = resolve_function_type(func, ctx) else {
        // Async/generator functions without annotations need Promise/Iterator return
        // modeling, which the current built-in type universe does not express yet.
        return;
    };
    let crate::types::Type::Function(function_type) = ctx.arena.get(signature).clone() else {
        return;
    };

    bind_params(&func.params, &function_type.params, scoping, ctx);
    let outer_narrow = std::mem::take(&mut ctx.narrow);
    let type_param_scope = ctx.namespace.push_type_params(&mut ctx.arena, func);
    let infers_return = func.return_type.is_none();
    let return_scope = ctx.enter_function_return_scope(
        Some(function_type.return_type),
        infers_return,
        func.r#async,
    );
    hoist_function_declarations(&body.statements, ctx);
    for body_stmt in &body.statements {
        check_statement(body_stmt, scoping, ctx);
    }
    let returned = ctx.leave_return_scope(return_scope);
    ctx.narrow = outer_narrow;

    let return_type = if infers_return {
        if returned.is_empty() {
            ctx.arena.void()
        } else {
            ctx.arena.alloc_union(returned)
        }
    } else {
        function_type.return_type
    };
    let inferred = ctx
        .arena
        .alloc(crate::types::Type::Function(crate::types::FunctionType {
            params: function_type.params,
            return_type,
            type_params: function_type.type_params,
            is_untyped: function_type.is_untyped,
        }));
    ctx.namespace.pop_type_params(type_param_scope);
    ctx.exported_function_types
        .insert(func.span().start, inferred);
}

// Declares every function declared directly in `statements` before any of them is
// checked, because function declarations hoist: `helper()` may be called above the
// line that declares it. Top-level functions were declared by the declare pass, so
// only a function without a type yet is declared here.
pub(super) fn hoist_function_declarations(
    statements: &[Statement],
    ctx: &mut CheckContext<'_, '_>,
) {
    for stmt in statements {
        if let Statement::FunctionDeclaration(func) = stmt
            && let Some(symbol_id) = func.id.as_ref().and_then(|id| id.symbol_id.get())
            && ctx.symbols.get(symbol_id).is_none()
        {
            declare_function(func, ctx);
        }
    }
}
