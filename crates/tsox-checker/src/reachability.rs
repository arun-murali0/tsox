// Which statements can never run, decided the way tsc's binder decides it.
//
// tsc marks a statement unreachable while it binds: every `return`, `throw`, `break` and
// `continue` leaves the "current flow" unreachable, and so does a loop or `if` whose exits
// are all closed off. The checker then reports one TS7027 for each run of adjacent dead
// statements. This module replays exactly that, over the oxc AST, and nothing else. It is
// arena-free and a pure function of the program, so the answer can be shared by anything that
// asks "is this statement dead": the TS7027 report today, definite assignment and the
// narrowing resolver later.
//
// Why not take it from oxc's control-flow graph? oxc builds a correct graph of JavaScript, and
// tsc's binder is not the same graph. The differences are small and every one of them changes
// which statements tsc calls dead:
//
//   * a condition is constant only when it is the bare `true` or `false` keyword; `(true)`,
//     `!true`, `1` and `null` are not, but `&&` and `||` pass constants through their operands
//     and `??` does not;
//   * `break` and `continue` jump straight to their target, past any enclosing `finally`;
//   * a `catch` and a `finally` are entered from the start of their `try`, so they are live
//     whenever the `try` is, whatever its body does;
//   * a `switch` without `default` always falls out of the bottom.
//
// oxc's graph knows none of these constants. So the reachability here is computed on the AST,
// and oxc's graph stays what it is good for (blocks and the node-to-block map) in the later
// phases. See docs/architecture/flow.md.
//
// Not covered, because they need types: a call to a function that returns `never`, an
// `asserts false` call, and a `switch` that exhausts its discriminant. tsc folds those into the
// same question at check time.

use oxc_ast::ast::{
    BindingPattern, Declaration, ExportDefaultDeclarationKind, Expression, ForStatementInit,
    ForStatementLeft, LabeledStatement, LogicalOperator, Program, Statement, SwitchStatement,
    TSNamespaceDeclaration, TSNamespaceDeclarationBody, TryStatement, UnaryOperator,
    VariableDeclarationKind,
};
use oxc_ast_visit::{Visit, walk};
use oxc_span::{GetSpan, Span};

/// The result of one pass over a file.
#[derive(Debug, Default)]
pub struct Reachability {
    /// Start offsets of every statement bound while the flow was unreachable. A function body
    /// inside dead code is its own container and starts reachable, so its statements are not
    /// here unless they are dead for their own reasons.
    pub dead: std::collections::HashSet<u32>,
    /// One span per TS7027: from the first statement of a run of dead statements to the end of
    /// the last. In source order within each statement list.
    pub reports: Vec<Span>,
    /// One span per `case` or `default` clause that has statements, is not the last clause, and
    /// can run off its end into the next one (TS7029, under `noFallthroughCasesInSwitch`). A
    /// clause with no statements is just a stacked label and is never one.
    pub fallthroughs: Vec<Span>,
}

impl Reachability {
    pub fn is_dead(&self, statement_start: u32) -> bool {
        self.dead.contains(&statement_start)
    }
}

pub fn analyze(program: &Program<'_>) -> Reachability {
    let mut analyzer = Analyzer::default();
    analyzer.visit_program(program);
    analyzer.out
}

// A jump target, kept as an index into `Analyzer::reached`: it is reached when some jump (or
// the flow itself) arrives there while reachable. tsc's `addAntecedent` ignores unreachable
// antecedents, so "has an antecedent" is exactly "was reached while reachable".
type Target = usize;

#[derive(Clone, Default)]
struct Jumps {
    break_target: Option<Target>,
    continue_target: Option<Target>,
    labels: Vec<Label>,
}

#[derive(Clone)]
struct Label {
    name: String,
    break_target: Target,
    continue_target: Option<Target>,
}

struct Analyzer {
    // Is the current flow reachable? tsc's `currentFlow !== unreachableFlow`.
    reach: bool,
    // Inside a statement that was reported (or is part of a reported run): nothing nested is
    // reported again. tsc's `withinUnreachableCode`.
    within: bool,
    jumps: Jumps,
    // Where a `return` goes: the exit label of the enclosing immediately-invoked function
    // (or the `finally` it passes through), if there is one. tsc's `currentReturnTarget`.
    return_target: Option<Target>,
    // Set by a call whose callee is a function expression, for the function visited next.
    iife_pending: bool,
    // The function that is the value of the class method being entered. tsc checks its body in
    // place rather than later, so it keeps `within`.
    in_place_function: Option<Span>,
    reached: Vec<bool>,
    // How many label statements sit directly around the statement being entered, so a loop can
    // make them its `continue` targets. tsc's `setContinueTarget`.
    pending_labels: usize,
    out: Reachability,
}

impl Default for Analyzer {
    fn default() -> Self {
        Self {
            reach: true,
            within: false,
            jumps: Jumps::default(),
            return_target: None,
            iife_pending: false,
            in_place_function: None,
            reached: Vec::new(),
            pending_labels: 0,
            out: Reachability::default(),
        }
    }
}

impl Analyzer {
    fn new_target(&mut self) -> Target {
        self.reached.push(false);
        self.reached.len() - 1
    }

    fn arrive(&mut self, target: Target) {
        if self.reach {
            self.reached[target] = true;
        }
    }

    // A function, class static block or namespace body is a flow container: it has no jump
    // targets of the code around it and says nothing about their labels.
    //
    // A function that is not immediately invoked starts reachable and leaves the surrounding
    // flow as it was. An immediately invoked one (`(() => { … })()`, not async, not a generator)
    // and a class static block do not start anything: the body continues the flow it is in, so
    // inside dead code it is dead, and what follows is reachable only if the body can return or
    // run off its end. tsc's `isImmediatelyInvoked` in `bindContainer`.
    //
    // `keep_within` is for a body the checker visits inline with its parent, a class static
    // block: inside a statement already reported as dead nothing is reported again. A function
    // body is checked later, on its own, so it starts clean.
    fn container(
        &mut self,
        immediately_invoked: bool,
        keep_within: bool,
        body: impl FnOnce(&mut Self),
    ) {
        let within = if keep_within {
            self.within
        } else {
            std::mem::replace(&mut self.within, false)
        };
        let jumps = std::mem::take(&mut self.jumps);
        let pending = std::mem::take(&mut self.pending_labels);
        let outer_return = self.return_target;
        if immediately_invoked {
            let exit = self.new_target();
            self.return_target = Some(exit);
            body(self);
            self.arrive(exit);
            self.reach = self.reached[exit];
        } else {
            let reach = std::mem::replace(&mut self.reach, true);
            self.return_target = None;
            body(self);
            self.reach = reach;
        }
        self.return_target = outer_return;
        self.within = within;
        self.jumps = jumps;
        self.pending_labels = pending;
    }

    // A statement that is not in a statement list (the arm of an `if`, a loop body, the body of
    // a label). tsc reports such a dead statement on its own, as a run of one.
    fn child<'a>(&mut self, statement: &Statement<'a>) {
        let dead = !self.reach;
        let report = dead && !self.within && reportable(statement);
        if report {
            self.out.reports.push(statement.span());
        }
        let within = self.within;
        if dead && is_unreachable_candidate(statement) {
            self.within = true;
        }
        self.visit_statement(statement);
        self.within = within;
    }

    // The two edges out of a condition, given whether it is reached: can the true edge be
    // taken, can the false edge. tsc's `createFlowCondition` makes an edge unreachable only
    // for the bare keyword, and `bindLogicalLikeExpression` threads the rest.
    fn condition(
        expression: &Expression<'_>,
        reach: bool,
        parent_is_nullish: bool,
    ) -> (bool, bool) {
        if !reach {
            return (false, false);
        }
        match expression {
            Expression::BooleanLiteral(literal) if !parent_is_nullish => {
                if literal.value {
                    (true, false)
                } else {
                    (false, true)
                }
            }
            Expression::LogicalExpression(logical) => {
                let nullish = logical.operator == LogicalOperator::Coalesce;
                let left = Self::condition(&logical.left, reach, nullish);
                match logical.operator {
                    LogicalOperator::And => {
                        let right = Self::condition(&logical.right, left.0, false);
                        (right.0, left.1 || right.1)
                    }
                    // `??` is bound like `||`, with the literal rule off for its operands.
                    LogicalOperator::Or | LogicalOperator::Coalesce => {
                        let right = Self::condition(&logical.right, left.1, nullish);
                        (left.0 || right.0, right.1)
                    }
                }
            }
            // Parentheses and `!` are looked through only around a logical expression; around
            // anything else the whole thing is one opaque condition, as in tsc.
            Expression::ParenthesizedExpression(inner) if is_logical(&inner.expression) => {
                Self::condition(&inner.expression, reach, parent_is_nullish)
            }
            Expression::UnaryExpression(unary)
                if unary.operator == UnaryOperator::LogicalNot && is_logical(&unary.argument) =>
            {
                let (when_true, when_false) = Self::condition(&unary.argument, reach, false);
                (when_false, when_true)
            }
            _ => (true, true),
        }
    }

    // `break` and `continue`: jump to the target when there is one, and leave the flow
    // unreachable. Without a target (invalid code) the flow carries on, as in tsc.
    fn jump(&mut self, label: Option<&str>, is_break: bool) {
        let target = match label {
            Some(name) => self
                .jumps
                .labels
                .iter()
                .rev()
                .find(|l| l.name == name)
                .and_then(|l| {
                    if is_break {
                        Some(l.break_target)
                    } else {
                        l.continue_target
                    }
                }),
            None => {
                if is_break {
                    self.jumps.break_target
                } else {
                    self.jumps.continue_target
                }
            }
        };
        if let Some(target) = target {
            self.arrive(target);
            self.reach = false;
        }
    }

    // Runs `body` with a loop's targets in force, and with its `continue` target given to the
    // labels directly around it.
    fn in_loop<'a>(
        &mut self,
        pending_labels: usize,
        break_target: Target,
        continue_target: Target,
        body: impl FnOnce(&mut Self),
    ) {
        let saved = self.jumps.clone();
        self.jumps.break_target = Some(break_target);
        self.jumps.continue_target = Some(continue_target);
        let labels = self.jumps.labels.len();
        for label in self.jumps.labels[labels.saturating_sub(pending_labels)..].iter_mut() {
            label.continue_target = Some(continue_target);
        }
        body(self);
        self.jumps = saved;
    }

    fn visit_binding_pattern_exprs<'a>(&mut self, pattern: &BindingPattern<'a>) {
        walk::walk_binding_pattern(self, pattern);
    }
}

impl<'a> Visit<'a> for Analyzer {
    fn visit_function(
        &mut self,
        function: &oxc_ast::ast::Function<'a>,
        flags: oxc_semantic::ScopeFlags,
    ) {
        let immediately_invoked =
            std::mem::take(&mut self.iife_pending) && !function.r#async && !function.generator;
        // The body of a class method, constructor or accessor is checked in place; every other
        // function body is checked later, on its own. See `container`.
        let in_place = self.in_place_function == Some(function.span);
        self.container(immediately_invoked, in_place, |this| {
            walk::walk_function(this, function, flags)
        });
    }

    fn visit_method_definition(&mut self, method: &oxc_ast::ast::MethodDefinition<'a>) {
        let saved = self.in_place_function.replace(method.value.span);
        walk::walk_method_definition(self, method);
        self.in_place_function = saved;
    }

    fn visit_arrow_function_expression(
        &mut self,
        arrow: &oxc_ast::ast::ArrowFunctionExpression<'a>,
    ) {
        let immediately_invoked = std::mem::take(&mut self.iife_pending) && !arrow.r#async;
        self.container(immediately_invoked, false, |this| {
            walk::walk_arrow_function_expression(this, arrow)
        });
    }

    fn visit_static_block(&mut self, block: &oxc_ast::ast::StaticBlock<'a>) {
        self.container(true, true, |this| this.visit_statements(&block.body));
    }

    // A namespace body is checked in place, as part of checking the declaration, so inside a
    // namespace that is itself dead nothing is reported again.
    fn visit_ts_module_block(&mut self, block: &oxc_ast::ast::TSModuleBlock<'a>) {
        self.container(false, true, |this| this.visit_statements(&block.body));
    }

    // A call whose callee is a function expression is the one case where the function runs
    // right here. The arguments are bound first, then the function, as in tsc.
    fn visit_call_expression(&mut self, call: &oxc_ast::ast::CallExpression<'a>) {
        let mut callee = &call.callee;
        while let Expression::ParenthesizedExpression(inner) = callee {
            callee = &inner.expression;
        }
        if !call.optional
            && matches!(
                callee,
                Expression::ArrowFunctionExpression(_) | Expression::FunctionExpression(_)
            )
        {
            for argument in &call.arguments {
                self.visit_argument(argument);
            }
            self.iife_pending = true;
            self.visit_expression(&call.callee);
            self.iife_pending = false;
        } else {
            walk::walk_call_expression(self, call);
        }
    }

    // The one hook every statement list goes through. Walks the list in order and groups the
    // dead statements the way tsc reports them: one diagnostic for each maximal run of adjacent
    // dead statements that can execute, ended by anything that cannot (a function declaration,
    // an empty statement, a `var` without initializer, a block, an import) or by a live one.
    fn visit_statements(&mut self, statements: &oxc_allocator::Vec<'a, Statement<'a>>) {
        let mut run: Option<Span> = None;
        for statement in statements {
            let dead = !self.reach;
            let candidate = dead && is_unreachable_candidate(statement);
            if candidate && !self.within {
                let span = statement.span();
                run = Some(match run {
                    Some(open) => Span::new(open.start, span.end),
                    None => span,
                });
            } else if let Some(open) = run.take() {
                self.out.reports.push(open);
            }
            let within = self.within;
            if candidate {
                self.within = true;
            }
            self.visit_statement(statement);
            self.within = within;
        }
        if let Some(open) = run.take() {
            self.out.reports.push(open);
        }
    }

    fn visit_statement(&mut self, statement: &Statement<'a>) {
        let entry = self.reach;
        let pending = std::mem::take(&mut self.pending_labels);
        if !entry {
            self.out.dead.insert(statement.span().start);
        }
        match statement {
            Statement::BlockStatement(block) => self.visit_statements(&block.body),
            Statement::ReturnStatement(statement) => {
                if let Some(argument) = &statement.argument {
                    self.visit_expression(argument);
                }
                if let Some(target) = self.return_target {
                    self.arrive(target);
                }
                self.reach = false;
            }
            Statement::ThrowStatement(statement) => {
                self.visit_expression(&statement.argument);
                self.reach = false;
            }
            Statement::BreakStatement(statement) => {
                self.jump(statement.label.as_ref().map(|l| l.name.as_str()), true);
            }
            Statement::ContinueStatement(statement) => {
                self.jump(statement.label.as_ref().map(|l| l.name.as_str()), false);
            }
            Statement::IfStatement(statement) => {
                self.visit_expression(&statement.test);
                let (when_true, when_false) = Self::condition(&statement.test, entry, false);
                self.reach = when_true;
                self.child(&statement.consequent);
                let after_then = self.reach;
                self.reach = when_false;
                if let Some(alternate) = &statement.alternate {
                    self.child(alternate);
                }
                self.reach = after_then || self.reach;
            }
            Statement::WhileStatement(statement) => {
                self.visit_expression(&statement.test);
                let (when_true, when_false) = Self::condition(&statement.test, entry, false);
                let post = self.new_target();
                let again = self.new_target();
                self.reach = when_true;
                self.in_loop(pending, post, again, |this| this.child(&statement.body));
                self.reach = when_false || self.reached[post];
            }
            Statement::DoWhileStatement(statement) => {
                let post = self.new_target();
                let before_test = self.new_target();
                self.reach = entry;
                self.in_loop(pending, post, before_test, |this| {
                    this.child(&statement.body);
                    this.arrive(before_test);
                });
                self.reach = self.reached[before_test];
                self.visit_expression(&statement.test);
                let (_, when_false) = Self::condition(&statement.test, self.reach, false);
                self.reach = when_false || self.reached[post];
            }
            Statement::ForStatement(statement) => {
                match &statement.init {
                    Some(ForStatementInit::VariableDeclaration(declaration)) => {
                        self.visit_variable_declaration(declaration)
                    }
                    Some(init) => walk::walk_for_statement_init(self, init),
                    None => {}
                }
                let (when_true, when_false) = match &statement.test {
                    // No condition: the true edge is the flow, the false edge is closed.
                    None => (entry, false),
                    Some(test) => {
                        self.visit_expression(test);
                        Self::condition(test, entry, false)
                    }
                };
                let post = self.new_target();
                let increment = self.new_target();
                self.reach = when_true;
                self.in_loop(pending, post, increment, |this| {
                    this.child(&statement.body);
                    this.arrive(increment);
                });
                self.reach = self.reached[increment];
                if let Some(update) = &statement.update {
                    self.visit_expression(update);
                }
                self.reach = when_false || self.reached[post];
            }
            Statement::ForInStatement(statement) => {
                self.visit_expression(&statement.right);
                self.visit_for_left(&statement.left);
                self.for_in_or_of(entry, pending, &statement.body);
            }
            Statement::ForOfStatement(statement) => {
                self.visit_expression(&statement.right);
                self.visit_for_left(&statement.left);
                self.for_in_or_of(entry, pending, &statement.body);
            }
            Statement::LabeledStatement(statement) => self.labeled(statement, pending),
            Statement::SwitchStatement(statement) => self.switch(statement, entry),
            Statement::TryStatement(statement) => self.try_statement(statement, entry),
            other => walk::walk_statement(self, other),
        }
    }
}

impl<'a> Analyzer {
    fn visit_for_left(&mut self, left: &ForStatementLeft<'a>) {
        walk::walk_for_statement_left(self, left);
    }

    // After a `for…in` or `for…of` the flow is reachable whenever the loop was entered: the
    // loop label feeds the exit directly, whatever the body does.
    fn for_in_or_of(&mut self, entry: bool, pending: usize, body: &Statement<'a>) {
        let post = self.new_target();
        let again = self.new_target();
        self.reach = entry;
        self.arrive(post);
        self.in_loop(pending, post, again, |this| this.child(body));
        self.reach = self.reached[post];
    }

    fn labeled(&mut self, statement: &LabeledStatement<'a>, pending: usize) {
        let post = self.new_target();
        self.jumps.labels.push(Label {
            name: statement.label.name.to_string(),
            break_target: post,
            continue_target: None,
        });
        self.pending_labels = pending + 1;
        self.child(&statement.body);
        self.pending_labels = 0;
        self.jumps.labels.pop();
        self.arrive(post);
        self.reach = self.reached[post];
    }

    // A `switch`. Every clause is entered from the top of the switch, so each is live whenever
    // the switch is; clauses fall into one another. The exit is reached by a `break`, by the end
    // of the last clause, and, with no `default`, by "no clause matched".
    fn switch(&mut self, statement: &SwitchStatement<'a>, entry: bool) {
        self.visit_expression(&statement.discriminant);
        let post = self.new_target();
        let saved = self.jumps.clone();
        self.jumps.break_target = Some(post);
        let mut end = entry;
        for (index, case) in statement.cases.iter().enumerate() {
            if let Some(test) = &case.test {
                self.visit_expression(test);
            }
            self.reach = entry;
            self.visit_statements(&case.consequent);
            end = self.reach;
            if self.reach && !case.consequent.is_empty() && index + 1 != statement.cases.len() {
                self.out.fallthroughs.push(case.span);
            }
        }
        self.reach = end;
        self.arrive(post);
        if !statement.cases.iter().any(|case| case.test.is_none()) {
            self.reach = entry;
            self.arrive(post);
        }
        self.jumps = saved;
        self.reach = self.reached[post];
    }

    // A `try`. The `catch` and the `finally` are entered from the start of the `try`, so both
    // are live whenever it is. What follows is live when the `finally` can finish and the `try`
    // or the `catch` can finish normally.
    fn try_statement(&mut self, statement: &TryStatement<'a>, entry: bool) {
        // With a `finally`, a `return` in the `try` or the `catch` goes to the `finally` first
        // and on to the outer return target only if the `finally` can finish.
        let outer_return = self.return_target;
        let through_finally = statement.finalizer.as_ref().map(|_| self.new_target());
        if through_finally.is_some() {
            self.return_target = through_finally;
        }
        self.reach = entry;
        self.visit_statements(&statement.block.body);
        let mut normal = self.reach;
        if let Some(handler) = &statement.handler {
            self.reach = entry;
            if let Some(param) = &handler.param {
                self.visit_binding_pattern_exprs(&param.pattern);
            }
            self.visit_statements(&handler.body.body);
            normal = normal || self.reach;
        }
        self.return_target = outer_return;
        match &statement.finalizer {
            Some(finalizer) => {
                self.reach = entry;
                self.visit_statements(&finalizer.body);
                let finished = self.reach;
                if let (Some(outer), Some(inner)) = (outer_return, through_finally) {
                    if finished && self.reached[inner] {
                        self.reached[outer] = true;
                    }
                }
                self.reach = finished && normal;
            }
            None => self.reach = normal,
        }
    }
}

// Looking through parentheses and `!`, is this a `&&`, `||` or `??`? tsc's
// `isLogicalExpression`: only then are the targets threaded through instead of making one
// condition out of the whole expression.
fn is_logical(expression: &Expression<'_>) -> bool {
    match expression {
        Expression::ParenthesizedExpression(inner) => is_logical(&inner.expression),
        Expression::UnaryExpression(unary) if unary.operator == UnaryOperator::LogicalNot => {
            is_logical(&unary.argument)
        }
        Expression::LogicalExpression(_) => true,
        _ => false,
    }
}

// tsc's `isPotentiallyExecutableNode`: the statements that start or extend a report. A
// declaration that only exists at compile time, a hoisted function, an empty statement, a
// block (whose children are judged one by one) and a `var` without an initializer are not.
fn is_potentially_executable(statement: &Statement<'_>) -> bool {
    if let Some(declaration) = statement.as_declaration() {
        return declaration_is_executable(declaration);
    }
    match statement {
        Statement::ExpressionStatement(_)
        | Statement::IfStatement(_)
        | Statement::DoWhileStatement(_)
        | Statement::WhileStatement(_)
        | Statement::ForStatement(_)
        | Statement::ForInStatement(_)
        | Statement::ForOfStatement(_)
        | Statement::ContinueStatement(_)
        | Statement::BreakStatement(_)
        | Statement::ReturnStatement(_)
        | Statement::WithStatement(_)
        | Statement::SwitchStatement(_)
        | Statement::LabeledStatement(_)
        | Statement::ThrowStatement(_)
        | Statement::TryStatement(_)
        | Statement::DebuggerStatement(_) => true,
        // `export const x = 1` and `export class C {}` are the declaration inside.
        Statement::ExportDeclaration(export) => declaration_is_executable(&export.declaration),
        Statement::ExportDefaultDeclaration(export) => matches!(
            export.declaration,
            ExportDefaultDeclarationKind::ClassDeclaration(_)
        ),
        _ => false,
    }
}

fn declaration_is_executable(declaration: &Declaration<'_>) -> bool {
    match declaration {
        Declaration::VariableDeclaration(declaration) => {
            declaration.kind != VariableDeclarationKind::Var
                || declaration.declarations.iter().any(|d| d.init.is_some())
        }
        Declaration::ClassDeclaration(_) | Declaration::TSEnumDeclaration(_) => true,
        Declaration::TSNamespaceDeclaration(namespace) => namespace_is_instantiated(namespace),
        _ => false,
    }
}

// A `const enum` is executable but never reported (tsc leaves it out unless preserveConstEnums
// is on), and it ends a run like any other statement that is not reported.
fn is_unreachable_candidate(statement: &Statement<'_>) -> bool {
    if !is_potentially_executable(statement) {
        return false;
    }
    let declaration = match statement {
        Statement::ExportDeclaration(export) => Some(&export.declaration),
        other => other.as_declaration(),
    };
    !matches!(declaration, Some(Declaration::TSEnumDeclaration(e)) if e.r#const)
}

fn reportable(statement: &Statement<'_>) -> bool {
    is_unreachable_candidate(statement)
}

// A namespace is instantiated, so it can run and can be dead, when something in it exists at
// run time. One holding only types, or an ambient one, is not.
fn namespace_is_instantiated(namespace: &TSNamespaceDeclaration<'_>) -> bool {
    if namespace.declare {
        return false;
    }
    match &namespace.body {
        TSNamespaceDeclarationBody::TSNamespaceDeclaration(inner) => {
            namespace_is_instantiated(inner)
        }
        TSNamespaceDeclarationBody::TSModuleBlock(block) => block.body.iter().any(|statement| {
            let declaration = match statement {
                Statement::ExportDeclaration(export) => Some(&export.declaration),
                other => other.as_declaration(),
            };
            match declaration {
                Some(
                    Declaration::TSInterfaceDeclaration(_) | Declaration::TSTypeAliasDeclaration(_),
                ) => false,
                Some(Declaration::TSEnumDeclaration(e)) => !e.r#const,
                Some(Declaration::TSNamespaceDeclaration(inner)) => {
                    namespace_is_instantiated(inner)
                }
                _ => true,
            }
        }),
    }
}
