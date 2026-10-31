use oxc_ast::ast::{
    Expression, IfStatement, Program, Statement, VariableDeclarationKind, WhileStatement,
};
use oxc_ast_visit::Visit;
use oxc_cfg::ControlFlowGraph;
use oxc_semantic::Semantic;
use oxc_span::GetSpan;

use super::context::CheckContext;
use super::statements::{contains_break, statement_always_exits};

// Reports code that can never run, once per contiguous dead region and at its first
// statement, which is what tsc's TS7027 does. Two sources of knowledge are combined.
//
// The control-flow graph. A statement whose own basic block the graph marks unreachable
// can never run: most commonly code after an unconditional `return`, `throw`, `break` or
// `continue`. Asking the graph rather than re-deriving reachability from the AST means
// every way the builder can prove a block dead is covered without special cases here.
//
// A few constant conditions the graph does not see. tsc's binder treats a literal `true`
// or `false` condition as known, and oxc's graph does not, so the walker applies the same
// rule itself, and only for those literals: `while (true)` with nothing that can `break`
// is never left, so the next statement is dead; `if (true)` makes its `else` dead, and
// makes what follows it dead when the branch exits; `if (false)` and `while (false)` make
// their body dead. Anything cleverer than a literal is deliberately not attempted, so a
// condition this does not recognise costs a missed report and never a false one.
//
// Every statement list in the file is visited, whatever holds it: function and method
// bodies, arrow and function expressions (including callbacks), constructors, accessors,
// static blocks, `try`/`catch`/`finally`, loops of every kind, labeled statements, switch
// cases and plain blocks. A statement list is where "the rest is dead" is decided, and a
// visitor reaches all of them through one hook, so no construct has to be listed.
//
// tsc does not report some statements when they are dead, and neither does this: function,
// interface and type alias declarations (hoisted, or without runtime effect), empty
// statements, `var` with no initializer, `const enum`, and import or export declarations.
// They are skipped over, and the next executable statement is the one reported.
//
// Verified against oxc's real graph, not assumed: a function declaration placed after an
// unconditional return is not flagged (see
// tests/unreachable_code.rs::an_unreachable_function_declaration_is_not_flagged).
pub(crate) fn check_unreachable_code<'a>(
    program: &Program<'a>,
    semantic: &Semantic<'a>,
    ctx: &mut CheckContext<'_, '_>,
) {
    // A missing graph means silently reporting nothing rather than panicking: this pass
    // only ever adds diagnostics, so losing it should not take the rest of the check
    // down with it. parse::analyze always builds the graph, so in practice this only
    // fires if that call site changes without this one noticing.
    let Some(cfg) = semantic.cfg() else {
        return;
    };
    let mut walker = Walker {
        semantic,
        cfg,
        ctx,
        reported: Vec::new(),
    };
    walker.visit_program(program);
}

struct Walker<'w, 'a, 'x, 'y> {
    semantic: &'w Semantic<'a>,
    cfg: &'w ControlFlowGraph,
    ctx: &'w mut CheckContext<'x, 'y>,
    // Start offsets already reported, so a statement found dead by both the graph and a
    // constant-condition rule is still reported once.
    reported: Vec<u32>,
}

impl Walker<'_, '_, '_, '_> {
    // Reports `stmt` as dead and says whether anything was reported. tsc never reports a
    // block itself, only the first executable statement inside it, so a dead block is
    // looked through; a block holding nothing but statements tsc skips reports nothing,
    // and the caller carries on to the next sibling.
    fn report_dead(&mut self, stmt: &Statement) -> bool {
        if let Statement::BlockStatement(block) = stmt {
            for inner in &block.body {
                if !is_not_reported_when_dead(inner) && self.report_dead(inner) {
                    return true;
                }
            }
            return false;
        }
        let span = stmt.span();
        if !self.ctx.options.allow_unreachable_code && !self.reported.contains(&span.start) {
            self.reported.push(span.start);
            self.ctx.error(
                crate::diagnostic_messages::messages::unreachable_code(),
                span,
            );
        }
        true
    }

    fn graph_says_unreachable(&self, stmt: &Statement) -> bool {
        let block = self.semantic.nodes().cfg_id(stmt.node_id());
        self.cfg.basic_block(block).is_unreachable()
    }
}

impl<'a> Visit<'a> for Walker<'_, 'a, '_, '_> {
    // The one hook every statement list goes through. Walks the list in order, reports
    // the first statement that is dead and stops there: everything after it shares its
    // dead block, so reporting the rest would only repeat the finding.
    fn visit_statements(&mut self, stmts: &oxc_allocator::Vec<'a, Statement<'a>>) {
        let mut dead_by_rule = false;
        for stmt in stmts {
            let dead = dead_by_rule || self.graph_says_unreachable(stmt);
            if dead {
                if !is_not_reported_when_dead(stmt) && self.report_dead(stmt) {
                    return;
                }
                // Skipped statements still get their own insides checked: a hoisted
                // function's body is reachable code in its own right.
                self.visit_statement(stmt);
                continue;
            }
            self.visit_statement(stmt);
            if ends_dead_by_rule(stmt) {
                dead_by_rule = true;
            }
        }
    }

    // `if (true) A else B` makes B dead and `if (false) A` makes A dead. The dead branch
    // is reported and not entered; the live one is walked normally.
    fn visit_if_statement(&mut self, it: &IfStatement<'a>) {
        self.visit_expression(&it.test);
        match literal_bool(&it.test) {
            Some(true) => {
                self.visit_statement(&it.consequent);
                if let Some(alternate) = &it.alternate {
                    self.report_dead(alternate);
                }
            }
            Some(false) => {
                self.report_dead(&it.consequent);
                if let Some(alternate) = &it.alternate {
                    self.visit_statement(alternate);
                }
            }
            None => {
                self.visit_statement(&it.consequent);
                if let Some(alternate) = &it.alternate {
                    self.visit_statement(alternate);
                }
            }
        }
    }

    fn visit_while_statement(&mut self, it: &WhileStatement<'a>) {
        self.visit_expression(&it.test);
        if literal_bool(&it.test) == Some(false) {
            self.report_dead(&it.body);
        } else {
            self.visit_statement(&it.body);
        }
    }
}

// A literal `true` or `false`, looking through parentheses. Nothing else counts as a
// known condition.
fn literal_bool(expr: &Expression) -> Option<bool> {
    match expr {
        Expression::BooleanLiteral(literal) => Some(literal.value),
        Expression::ParenthesizedExpression(inner) => literal_bool(&inner.expression),
        _ => None,
    }
}

// Whether control can never continue past `stmt` because of a constant condition the graph
// does not see. A loop that nothing can `break` out of is never left; the `break` scan is
// generous (see contains_break), so a missed loop exit only skips a report.
fn ends_dead_by_rule(stmt: &Statement) -> bool {
    match stmt {
        Statement::WhileStatement(inner) => {
            literal_bool(&inner.test) == Some(true) && !contains_break(&inner.body)
        }
        Statement::DoWhileStatement(inner) => {
            literal_bool(&inner.test) == Some(true) && !contains_break(&inner.body)
        }
        Statement::ForStatement(inner) => {
            inner
                .test
                .as_ref()
                .is_none_or(|test| literal_bool(test) == Some(true))
                && !contains_break(&inner.body)
        }
        Statement::IfStatement(inner) => match literal_bool(&inner.test) {
            Some(true) => statement_always_exits(&inner.consequent),
            Some(false) => inner.alternate.as_ref().is_some_and(statement_always_exits),
            None => false,
        },
        _ => false,
    }
}

// The statements tsc leaves alone when they are dead. See the module comment.
fn is_not_reported_when_dead(stmt: &Statement) -> bool {
    match stmt {
        Statement::FunctionDeclaration(_)
        | Statement::TSInterfaceDeclaration(_)
        | Statement::TSTypeAliasDeclaration(_)
        | Statement::EmptyStatement(_)
        | Statement::ImportDeclaration(_)
        | Statement::ExportNamedDeclaration(_)
        | Statement::ExportDefaultDeclaration(_)
        | Statement::ExportAllDeclaration(_) => true,
        Statement::VariableDeclaration(decl) => {
            decl.kind == VariableDeclarationKind::Var
                && decl.declarations.iter().all(|d| d.init.is_none())
        }
        Statement::TSEnumDeclaration(decl) => decl.r#const,
        _ => false,
    }
}
