use oxc_ast::ast::{Declaration, ExportDefaultDeclarationKind, Expression, Program, Statement};
use oxc_semantic::Scoping;
use oxc_span::GetSpan;

use super::context::CheckContext;
use super::expressions::{check_excess_properties, infer_expression_type, report_mismatch};

mod classes;
mod control_flow;
mod functions;
mod patterns;
mod support;
mod variables;

pub(super) use patterns::{bind_params, bind_pattern};
pub(super) use support::{contains_break, statement_always_exits, statement_leaves_flow};

use classes::check_class_declaration;
use control_flow::{
    check_do_while_statement, check_for_in_statement, check_for_of_statement, check_for_statement,
    check_if_statement, check_labeled_statement, check_switch_statement, check_try_statement,
    check_while_statement, record_break, record_continue,
};
use functions::{check_function_declaration, hoist_function_declarations};
use variables::check_variable_declaration;

pub fn check_top_level(program: &Program, scoping: &Scoping, ctx: &mut CheckContext<'_, '_>) {
    // Where every variable is written, found before any statement is checked: loops and
    // try blocks ask which variables they assign, and closures whether a variable is
    // assigned after they are made.
    ctx.writes = super::narrow::WriteMap::collect(program, scoping);
    for stmt in &program.body {
        check_statement(stmt, scoping, ctx);
    }
}

// True when `expr`, parentheses aside, is a call by name to the function whose return type
// is being inferred.
fn returns_call_to_owner(expr: &Expression, scoping: &Scoping, ctx: &CheckContext<'_, '_>) -> bool {
    let Some(owner) = ctx.return_reads.owner else {
        return false;
    };
    let Expression::CallExpression(call) = expr.without_parentheses() else {
        return false;
    };
    let Expression::Identifier(callee) = &call.callee else {
        return false;
    };
    super::narrow::resolve_symbol_id(callee, scoping) == Some(owner)
}

pub(super) fn check_return_statement(
    ret: &oxc_ast::ast::ReturnStatement,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) {
    // undefined for a bare `return;`, matching what a function that falls off the
    // end (or returns with no value) actually produces at runtime.
    let actual = match &ret.argument {
        Some(expr) => {
            ctx.contextual = ctx.current_return_type;
            // tsc leaves a bare call to the function itself out of its return type, so
            // `return gcd(b, a % b)` reads nothing and is not a circle.
            let reads = !returns_call_to_owner(expr, scoping, ctx);
            let outer = std::mem::replace(&mut ctx.return_reads.in_return_expression, reads);
            let actual = infer_expression_type(expr, scoping, ctx);
            ctx.return_reads.in_return_expression = outer;
            actual
        }
        None => ctx.arena.undefined(),
    };

    // A function with no return annotation learns its return type from what its body
    // returns; the type is widened so `return 1` makes the function return number,
    // not the literal 1, the way tsc infers it.
    if ctx.inferred_returns.is_some() {
        let widened = crate::types::widen(&ctx.arena, actual);
        if let Some(returns) = ctx.inferred_returns.as_mut() {
            returns.push(widened);
        }
    }

    // In an async function what is returned is awaited, so `return promise` is a T.
    let actual = if ctx.current_return_async {
        ctx.awaited_type(actual)
    } else {
        actual
    };

    if let Some(expected) = ctx.current_return_type {
        if !ctx.is_assignable_instantiating(actual, expected) {
            let whole = crate::diagnostic_messages::messages::return_type_mismatch(
                &ctx.arena, actual, expected,
            );
            report_mismatch(
                ret.argument.as_ref(),
                actual,
                expected,
                whole,
                ret.span(),
                ctx,
            );
        } else if let Some(argument) = &ret.argument {
            check_excess_properties(argument, expected, ctx);
        }
    }
}

pub(super) fn check_expression_statement(
    expr_stmt: &oxc_ast::ast::ExpressionStatement,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) {
    infer_expression_type(&expr_stmt.expression, scoping, ctx);
}

// The single dispatch point for every statement kind this checker understands,
// mirroring infer_expression_type in expressions/mod.rs. Type-only declarations
// (interfaces, aliases, enums) have already done their work in the earlier
// declare pass and need no body-checking here, so they are explicitly matched to
// a no-op rather than falling through to the unsupported-statement warning.
pub(crate) fn check_statement(stmt: &Statement, scoping: &Scoping, ctx: &mut CheckContext<'_, '_>) {
    if ctx.should_stop() {
        return;
    }
    match stmt {
        Statement::TSTypeAliasDeclaration(_)
        | Statement::TSInterfaceDeclaration(_)
        | Statement::TSEnumDeclaration(_) => {}
        Statement::TSNamespaceDeclaration(ns) => check_namespace(ns, scoping, ctx),
        // `declare module "x" { ... }`: the block is checked with `x` as its scope, so a
        // name written in it is found. A shorthand `declare module "x";` has no body.
        Statement::TSExternalModuleDeclaration(module) => {
            if let Some(body) = &module.body {
                ctx.namespace
                    .push_scope(&super::declare::module_scope(&module.id.value));
                // An external module declaration is ambient by nature, so everything in
                // its body is too (see CheckContext::ambient_depth).
                ctx.ambient_depth += 1;
                for statement in &body.body {
                    check_statement(statement, scoping, ctx);
                }
                ctx.ambient_depth -= 1;
                ctx.namespace.pop_scope();
            }
        }
        // Neither has a type of its own to check, but each one carries the narrowing in
        // force where it happens to the loop, switch or label it jumps to, so the code
        // after that construct (or the next pass of the loop) can join it in. A labelled
        // `break outer;` is still just this variant with a label attached.
        Statement::BreakStatement(jump) => {
            record_break(jump.label.as_ref().map(|label| label.name.as_str()), ctx)
        }
        Statement::ContinueStatement(jump) => {
            record_continue(jump.label.as_ref().map(|label| label.name.as_str()), ctx)
        }
        // The thrown value is an expression like any other; what a throw does to the
        // code after it is statement_always_exits' and statement_leaves_flow's business.
        Statement::ThrowStatement(throw) => {
            infer_expression_type(&throw.argument, scoping, ctx);
        }
        Statement::BlockStatement(block) => {
            // Deliberately no save/restore of ctx.narrow here, despite variable
            // declarations genuinely being block-scoped: TypeScript's narrowing
            // is not. It follows control flow, not lexical scope, and a bare
            // `{ ... }` has no effect on control flow at all -- it's not a
            // branch, a loop, or an exit. A guard clause's narrowing does
            // survive past a bare block's closing brace in real TypeScript
            // (verified against tsc; see
            // tests/fixtures/narrowing-scopes/guard_clause_in_block_does_survive.ts),
            // unlike the while/for/switch/function cases nearby, each of which
            // really is a distinct control-flow construct.
            check_block_statements(&block.body, scoping, ctx);
        }
        Statement::IfStatement(if_stmt) => check_if_statement(if_stmt, scoping, ctx),
        Statement::VariableDeclaration(decl) => check_variable_declaration(decl, scoping, ctx),
        Statement::FunctionDeclaration(func) => check_function_declaration(func, scoping, ctx),
        Statement::ReturnStatement(ret) => check_return_statement(ret, scoping, ctx),
        Statement::ExpressionStatement(expr_stmt) => {
            check_expression_statement(expr_stmt, scoping, ctx)
        }
        Statement::ClassDeclaration(class) => check_class_declaration(class, scoping, ctx),
        Statement::WhileStatement(while_stmt) => check_while_statement(while_stmt, scoping, ctx),
        Statement::ForStatement(for_stmt) => check_for_statement(for_stmt, scoping, ctx),
        Statement::DoWhileStatement(do_while) => check_do_while_statement(do_while, scoping, ctx),
        Statement::ForOfStatement(for_of) => check_for_of_statement(for_of, scoping, ctx),
        Statement::ForInStatement(for_in) => check_for_in_statement(for_in, scoping, ctx),
        Statement::TryStatement(try_stmt) => check_try_statement(try_stmt, scoping, ctx),
        Statement::LabeledStatement(labeled) => check_labeled_statement(labeled, scoping, ctx),
        Statement::SwitchStatement(switch_stmt) => {
            check_switch_statement(switch_stmt, scoping, ctx)
        }
        Statement::EmptyStatement(_) | Statement::DebuggerStatement(_) => {}
        // Problem: `export function f() {}` and friends were an unsupported statement,
        // so an exported declaration was neither declared nor checked, and everything
        // in a module that exports its API went unchecked.
        // Picked: look through the export to the declaration it wraps and check that
        // exactly as if it were not exported. A re-export list (`export { a }`,
        // `export * from "m"`) holds no code of its own, so it needs nothing here.
        Statement::ExportDeclaration(export) => {
            check_declaration(&export.declaration, scoping, ctx)
        }
        Statement::ExportDefaultDeclaration(export) => match &export.declaration {
            ExportDefaultDeclarationKind::FunctionDeclaration(func) => {
                check_function_declaration(func, scoping, ctx)
            }
            ExportDefaultDeclarationKind::ClassDeclaration(class) => {
                check_class_declaration(class, scoping, ctx)
            }
            ExportDefaultDeclarationKind::TSInterfaceDeclaration(_) => {}
            other => {
                if let Some(expr) = other.as_expression() {
                    infer_expression_type(expr, scoping, ctx);
                }
            }
        },
        Statement::ExportNamedDeclaration(_)
        | Statement::ExportFromDeclaration(_)
        | Statement::ExportAllDeclaration(_) => {}
        // With an import source the declaration was bound before checking started
        // (bridge::imports), so it is handled, not unsupported.
        Statement::ImportDeclaration(_) if ctx.imports.is_some() => {}
        other => support::push_unsupported(other, ctx),
    }
}

// `namespace N { ... }`: its statements are checked with `N` as the enclosing scope, so a
// sibling named without the prefix is found. `namespace A.B { ... }` is A with B inside it,
// so B's statements are checked with both as enclosing scopes, B innermost.
fn check_namespace(
    ns: &oxc_ast::ast::TSNamespaceDeclaration,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) {
    use oxc_ast::ast::TSNamespaceDeclarationBody;
    ctx.namespace.push_scope(&ns.id.name);
    // `declare namespace N { }` makes every declaration inside it ambient. A nested
    // `namespace A.B` is checked through the recursion below, inside this same count.
    let ambient = ns.declare;
    if ambient {
        ctx.ambient_depth += 1;
    }
    match &ns.body {
        TSNamespaceDeclarationBody::TSModuleBlock(block) => {
            for statement in &block.body {
                check_statement(statement, scoping, ctx);
            }
        }
        TSNamespaceDeclarationBody::TSNamespaceDeclaration(inner) => {
            check_namespace(inner, scoping, ctx)
        }
    }
    if ambient {
        ctx.ambient_depth -= 1;
    }
    ctx.namespace.pop_scope();
}

// The declaration behind an `export`. Type-only kinds did their work in the declare
// pass, the same as when they are not exported.
fn check_declaration(declaration: &Declaration, scoping: &Scoping, ctx: &mut CheckContext<'_, '_>) {
    match declaration {
        Declaration::VariableDeclaration(decl) => check_variable_declaration(decl, scoping, ctx),
        Declaration::FunctionDeclaration(func) => check_function_declaration(func, scoping, ctx),
        Declaration::ClassDeclaration(class) => check_class_declaration(class, scoping, ctx),
        Declaration::TSTypeAliasDeclaration(_)
        | Declaration::TSInterfaceDeclaration(_)
        | Declaration::TSEnumDeclaration(_) => {}
        Declaration::TSNamespaceDeclaration(ns) => check_namespace(ns, scoping, ctx),
        other => ctx.warning(
            crate::diagnostic_messages::messages::unimplemented_statement_kind("Other"),
            other.span(),
        ),
    }
}

// A statement list that opens a scope: a block, a try/catch/finally section. Function
// declarations in it hoist, so they are declared before any statement is checked.
pub(super) fn check_block_statements(
    statements: &[Statement],
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) {
    hoist_function_declarations(statements, ctx);
    for stmt in statements {
        check_statement(stmt, scoping, ctx);
    }
}
