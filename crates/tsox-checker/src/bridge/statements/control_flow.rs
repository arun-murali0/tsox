use oxc_ast::ast::{
    DoWhileStatement, Expression, ForInStatement, ForOfStatement, ForStatement, ForStatementLeft,
    IfStatement, LabeledStatement, Statement, SwitchStatement, TryStatement, WhileStatement,
};
use oxc_semantic::Scoping;
use oxc_span::{GetSpan, Span};

use crate::arena::{TypeArena, TypeId};
use crate::types::Type;

use super::super::context::CheckContext;
use super::super::expressions::infer_expression_type;
use super::super::narrow::{
    FrameKind, JumpFrame, NarrowState, join_states, narrow_condition, narrow_switch_case,
    narrow_switch_default, resolve_symbol_id, switch_discriminant,
};
use super::{
    bind_pattern, check_block_statements, check_statement, check_variable_declaration,
    statement_leaves_flow,
};

// Where control flow joins after an `if`. What the code after the statement sees
// depends on which branches can fall out the bottom, because a branch that always
// exits contributes nothing to it; that is why the guard clause
// `if (x === null) return;` narrows everything that follows. When both branches
// can fall through their end states are joined, so a variable narrowed or assigned
// on both keeps the union of the two, while one narrowed on only one branch
// reverts to its earlier type (`if (x === null) { x = "d"; }` leaves x a string).
// When neither can, the code after is unreachable and the earlier state is kept.
// With no else branch the missing alternate is the condition's false side alone.
pub(super) fn check_if_statement(
    if_stmt: &IfStatement,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) {
    infer_expression_type(&if_stmt.test, scoping, ctx);

    let (true_overrides, false_overrides) = narrow_condition(&if_stmt.test, scoping, ctx);
    let outer_narrow = ctx.narrow.clone();

    ctx.narrow.extend(true_overrides);
    check_statement(&if_stmt.consequent, scoping, ctx);
    let consequent_always_exits = statement_leaves_flow(&if_stmt.consequent);
    let consequent_end = std::mem::replace(&mut ctx.narrow, outer_narrow.clone());

    let (alternate_always_exits, alternate_end) = match &if_stmt.alternate {
        Some(alternate) => {
            ctx.narrow.extend(false_overrides);
            check_statement(alternate, scoping, ctx);
            (statement_leaves_flow(alternate), ctx.narrow.clone())
        }
        None => {
            let mut implicit_else = outer_narrow.clone();
            implicit_else.extend(false_overrides);
            (false, implicit_else)
        }
    };

    ctx.narrow = match (consequent_always_exits, alternate_always_exits) {
        (true, false) => alternate_end,
        (false, true) => consequent_end,
        (false, false) => join_states(&consequent_end, &alternate_end, &mut ctx.arena),
        (true, true) => outer_narrow,
    };
}

// How many times a loop is walked silently, looking for the state its head settles on,
// before giving up and forgetting what the loop assigns.
const MAX_HEAD_PASSES: usize = 3;

// What one walk over a loop produced.
struct LoopPass {
    // The state that flows into the next iteration, None when nothing loops back (the body
    // always leaves and nothing `continue`s).
    back: Option<NarrowState>,
    // The state when the loop ends by its own condition (test false, iterator exhausted),
    // None for a loop only a `break` or `return` can leave.
    exit: Option<NarrowState>,
    // The state at every `break` that leaves this loop.
    breaks: Vec<NarrowState>,
}

fn join_all(states: Vec<NarrowState>, arena: &mut TypeArena) -> Option<NarrowState> {
    let mut iter = states.into_iter();
    let first = iter.next()?;
    Some(iter.fold(first, |joined, next| join_states(&joined, &next, arena)))
}

fn is_constant_true(expr: &Expression) -> bool {
    match expr {
        Expression::BooleanLiteral(literal) => literal.value,
        Expression::ParenthesizedExpression(inner) => is_constant_true(&inner.expression),
        _ => false,
    }
}

// Walks a loop body once inside its own jump frame. Returns the state at the end of the
// body (None if the body cannot fall off its end), and the states at its `continue`s and
// `break`s.
fn walk_loop_body(
    body: &Statement,
    label: Option<String>,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> (Option<NarrowState>, Vec<NarrowState>, Vec<NarrowState>) {
    ctx.jump_frames.push(JumpFrame::new(label, FrameKind::Loop));
    check_statement(body, scoping, ctx);
    let (breaks, continues) = match ctx.jump_frames.pop() {
        Some(frame) => (frame.breaks, frame.continues),
        None => (Vec::new(), Vec::new()),
    };
    let end = if statement_leaves_flow(body) {
        None
    } else {
        Some(ctx.narrow.clone())
    };
    (end, continues, breaks)
}

// The state a pass flows back to the loop head with: wherever the body ended, or a
// `continue` jumped from.
fn back_edge(
    end: Option<NarrowState>,
    continues: Vec<NarrowState>,
    arena: &mut TypeArena,
) -> Option<NarrowState> {
    let mut states = continues;
    states.extend(end);
    join_all(states, arena)
}

// Walks a loop and leaves ctx.narrow as what the code after it sees.
//
// Problem: the body used to be walked once, from the state before the loop, so what the
// body assigns never reached the top of the next pass. After `let x: string | null =
// "a"; while (go) { x.length; x = null; }` the read looked safe, though from the second
// pass on x is null.
// Picked: the state at the loop head is the join of the state before the loop and the
// state flowing back from the end of the body (and from each `continue`). When the loop
// assigns something that is narrowed on entry, that is found by walking the body again
// from the joined state, silently, until it stops changing, and then one more time for
// real. A loop that assigns nothing narrowed is walked once, as before.
// The state after the loop is the join of the normal exit (the test failing, narrowed by
// the test's false side) and every `break`, each with the narrowing it had where it left.
// Cost: a loop that assigns a narrowed variable is walked up to MAX_HEAD_PASSES + 1 times,
// and nested loops multiply that, which is why the silent passes are skipped when nothing
// in the loop writes to a narrowed variable.
fn run_loop(
    ctx: &mut CheckContext<'_, '_>,
    span: Span,
    mut pass: impl FnMut(&mut CheckContext<'_, '_>) -> LoopPass,
) {
    let outer = ctx.narrow.clone();
    let written = ctx.writes.written_in(span.start, span.end);
    let needs_head_passes = written.iter().any(|&symbol_id| outer.mentions(symbol_id));

    let mut head = outer.clone();
    if needs_head_passes {
        let mut settled = false;
        for _ in 0..MAX_HEAD_PASSES {
            ctx.narrow = head.clone();
            // The same body is walked again for the real pass below, so what these
            // passes report would otherwise be reported twice.
            let reported_so_far = ctx.diagnostics.len();
            let result = pass(ctx);
            ctx.diagnostics.truncate(reported_so_far);
            let next = match result.back {
                Some(back) => join_states(&outer, &back, &mut ctx.arena),
                None => outer.clone(),
            };
            if next.same_as(&head) {
                settled = true;
                break;
            }
            head = next;
        }
        if !settled {
            // Not settled: whatever the loop assigns could be anything at the head.
            for &symbol_id in &written {
                head.remove(symbol_id);
            }
        }
    }

    ctx.narrow = head.clone();
    let result = pass(ctx);
    let mut leaving = Vec::new();
    leaving.extend(result.exit);
    leaving.extend(result.breaks);
    ctx.narrow = join_all(leaving, &mut ctx.arena).unwrap_or(head);
}

// A `while (true)` cannot leave through its test, so the only way past it is a `break`
// and the state after the loop is the join of the breaks alone. Any other test also lets
// the loop leave on its false side, with that side's narrowing.
pub(super) fn check_while_statement(
    while_stmt: &WhileStatement,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) {
    let label = ctx.pending_loop_label.take();
    let never_exits_by_test = is_constant_true(&while_stmt.test);
    run_loop(ctx, while_stmt.span(), |ctx| {
        infer_expression_type(&while_stmt.test, scoping, ctx);
        let (true_overrides, false_overrides) = narrow_condition(&while_stmt.test, scoping, ctx);
        let at_test = ctx.narrow.clone();
        ctx.narrow.extend(true_overrides);
        let (end, continues, breaks) =
            walk_loop_body(&while_stmt.body, label.clone(), scoping, ctx);
        let back = back_edge(end, continues, &mut ctx.arena);
        let exit = if never_exits_by_test {
            None
        } else {
            let mut leaving = at_test;
            leaving.extend(false_overrides);
            Some(leaving)
        };
        LoopPass { back, exit, breaks }
    });
}

pub(super) fn check_for_statement(
    for_stmt: &ForStatement,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) {
    if let Some(oxc_ast::ast::ForStatementInit::VariableDeclaration(decl)) = &for_stmt.init {
        check_variable_declaration(decl, scoping, ctx);
    }

    // A `for` with no test can only be left by a break or a return.
    let label = ctx.pending_loop_label.take();
    let never_exits_by_test = match &for_stmt.test {
        Some(test) => is_constant_true(test),
        None => true,
    };
    run_loop(ctx, for_stmt.span(), |ctx| {
        let (true_overrides, false_overrides) = match &for_stmt.test {
            Some(test) => {
                infer_expression_type(test, scoping, ctx);
                let (on_true, on_false) = narrow_condition(test, scoping, ctx);
                (on_true, Some(on_false))
            }
            None => (NarrowState::new(), None),
        };
        let at_test = ctx.narrow.clone();
        ctx.narrow.extend(true_overrides);
        let (end, continues, breaks) = walk_loop_body(&for_stmt.body, label.clone(), scoping, ctx);

        // The update runs at the end of every pass, from the state the pass ended in.
        let back = match back_edge(end, continues, &mut ctx.arena) {
            Some(state) => {
                ctx.narrow = state;
                if let Some(update) = &for_stmt.update {
                    infer_expression_type(update, scoping, ctx);
                }
                Some(ctx.narrow.clone())
            }
            None => {
                if let Some(update) = &for_stmt.update {
                    infer_expression_type(update, scoping, ctx);
                }
                None
            }
        };
        let exit = if never_exits_by_test {
            None
        } else {
            let mut leaving = at_test;
            if let Some(false_overrides) = false_overrides {
                leaving.extend(false_overrides);
            }
            Some(leaving)
        };
        LoopPass { back, exit, breaks }
    });
}

// The body runs before the test does, so the test cannot narrow the body; it narrows what
// the next pass and the code after the loop see.
pub(super) fn check_do_while_statement(
    do_while: &DoWhileStatement,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) {
    let label = ctx.pending_loop_label.take();
    let never_exits_by_test = is_constant_true(&do_while.test);
    run_loop(ctx, do_while.span(), |ctx| {
        let (end, continues, breaks) = walk_loop_body(&do_while.body, label.clone(), scoping, ctx);
        match back_edge(end, continues, &mut ctx.arena) {
            Some(at_test) => {
                ctx.narrow = at_test.clone();
                infer_expression_type(&do_while.test, scoping, ctx);
                let (true_overrides, false_overrides) =
                    narrow_condition(&do_while.test, scoping, ctx);
                let mut back = at_test.clone();
                back.extend(true_overrides);
                let exit = if never_exits_by_test {
                    None
                } else {
                    let mut leaving = at_test;
                    leaving.extend(false_overrides);
                    Some(leaving)
                };
                LoopPass {
                    back: Some(back),
                    exit,
                    breaks,
                }
            }
            None => {
                // The test is never reached; it is still checked.
                infer_expression_type(&do_while.test, scoping, ctx);
                LoopPass {
                    back: None,
                    exit: None,
                    breaks,
                }
            }
        }
    });
}

// What `for (const x of xs)` binds x to. An array gives its element type; a string gives
// string; anything else (a Set, a Map, a generator, a union) is not modelled and gives
// `any`, so a gap in the model never becomes an error on the source.
fn for_of_element_type(iterable: TypeId, ctx: &mut CheckContext<'_, '_>) -> TypeId {
    if let Type::Tuple(elements) = ctx.arena.get(iterable).clone() {
        // Whatever any of the elements is; an optional one is not skipped, it is a value
        // (or `undefined`) when the loop reaches it.
        return super::super::expressions::tuple_element_union(&mut ctx.arena, &elements, true);
    }
    match ctx.arena.get(iterable) {
        Type::Array(element) => *element,
        _ if iterable == ctx.arena.string() => ctx.arena.string(),
        _ => ctx.arena.any(),
    }
}

pub(super) fn check_for_of_statement(
    for_of: &ForOfStatement,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) {
    let iterable = infer_expression_type(&for_of.right, scoping, ctx);
    let element = for_of_element_type(iterable, ctx);
    check_for_each(
        for_of.span(),
        &for_of.left,
        &for_of.body,
        element,
        scoping,
        ctx,
    );
}

pub(super) fn check_for_in_statement(
    for_in: &ForInStatement,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) {
    infer_expression_type(&for_in.right, scoping, ctx);
    let key = ctx.arena.string();
    check_for_each(for_in.span(), &for_in.left, &for_in.body, key, scoping, ctx);
}

// `for (const x of xs)` and `for (const k in o)`. The loop ends when the sequence runs out,
// which says nothing about any variable, so the exit state is the head state itself.
fn check_for_each(
    span: Span,
    left: &ForStatementLeft,
    body: &Statement,
    element: TypeId,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) {
    let label = ctx.pending_loop_label.take();
    run_loop(ctx, span, |ctx| {
        let at_head = ctx.narrow.clone();
        bind_for_left(left, element, scoping, ctx);
        let (end, continues, breaks) = walk_loop_body(body, label.clone(), scoping, ctx);
        let back = back_edge(end, continues, &mut ctx.arena);
        LoopPass {
            back,
            exit: Some(at_head),
            breaks,
        }
    });
}

fn bind_for_left(
    left: &ForStatementLeft,
    element: TypeId,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) {
    match left {
        ForStatementLeft::VariableDeclaration(decl) => {
            for declarator in &decl.declarations {
                bind_pattern(&declarator.id, element, scoping, ctx);
            }
        }
        // `for (x of xs)` assigns an existing variable: whatever was known about it ends.
        ForStatementLeft::AssignmentTargetIdentifier(ident) => {
            if let Some(symbol_id) = resolve_symbol_id(ident, scoping) {
                ctx.narrow.remove(symbol_id);
            }
        }
        _ => {}
    }
}

// Where a `break` or `continue` goes: the innermost loop or switch for a bare `break`, the
// innermost loop for a bare `continue`, or the construct carrying the label.
pub(super) fn record_break(label: Option<&str>, ctx: &mut CheckContext<'_, '_>) {
    let target = ctx.jump_frames.iter().rposition(|frame| match label {
        Some(label) => frame.label.as_deref() == Some(label),
        None => matches!(frame.kind, FrameKind::Loop | FrameKind::Switch),
    });
    if let Some(index) = target {
        let state = ctx.narrow.clone();
        ctx.jump_frames[index].breaks.push(state);
    }
}

// A `continue` targets the innermost loop, or the loop its label names, never a switch or
// a labeled block. The search runs from the innermost frame outward, and the narrowing in
// force at the jump is kept on that loop's frame for the head of its next pass.
pub(super) fn record_continue(label: Option<&str>, ctx: &mut CheckContext<'_, '_>) {
    let target = ctx.jump_frames.iter().rposition(|frame| {
        frame.kind == FrameKind::Loop
            && match label {
                Some(label) => frame.label.as_deref() == Some(label),
                None => true,
            }
    });
    if let Some(index) = target {
        let state = ctx.narrow.clone();
        ctx.jump_frames[index].continues.push(state);
    }
}

// `outer: while (...)`. A loop directly inside takes the label so `continue outer` and
// `break outer` find it; any other body is a block only `break outer` can leave.
pub(super) fn check_labeled_statement(
    labeled: &LabeledStatement,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) {
    let label = labeled.label.name.to_string();
    if matches!(
        &labeled.body,
        Statement::WhileStatement(_)
            | Statement::DoWhileStatement(_)
            | Statement::ForStatement(_)
            | Statement::ForInStatement(_)
            | Statement::ForOfStatement(_)
    ) {
        ctx.pending_loop_label = Some(label.clone());
    }
    ctx.jump_frames
        .push(JumpFrame::new(Some(label), FrameKind::Label));
    check_statement(&labeled.body, scoping, ctx);
    let frame = ctx.jump_frames.pop();
    if let Some(frame) = frame
        && !frame.breaks.is_empty()
    {
        let mut states = frame.breaks;
        states.push(ctx.narrow.clone());
        if let Some(joined) = join_all(states, &mut ctx.arena) {
            ctx.narrow = joined;
        }
    }
}

// try / catch / finally.
//
// Any statement of the try block can throw, so the catch block starts from the state
// before the try with everything the try block assigns forgotten, not from the state the
// try block ended in. The code after the statement sees the join of the paths that fall
// out of the try block and the catch block. A finally block runs on every path, so it
// starts from the same forgetting of both, and what it assigns is forgotten afterwards.
pub(super) fn check_try_statement(
    try_stmt: &TryStatement,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) {
    let outer = ctx.narrow.clone();

    check_block_statements(&try_stmt.block.body, scoping, ctx);
    let mut falling_out = Vec::new();
    if !try_stmt
        .block
        .body
        .last()
        .is_some_and(statement_leaves_flow)
    {
        falling_out.push(ctx.narrow.clone());
    }
    let try_span = try_stmt.block.span();
    let written_in_try = ctx.writes.written_in(try_span.start, try_span.end);

    let mut written_in_handler = Vec::new();
    if let Some(handler) = &try_stmt.handler {
        let mut at_catch = outer.clone();
        for &symbol_id in &written_in_try {
            at_catch.remove(symbol_id);
        }
        ctx.narrow = at_catch;
        if let Some(param) = &handler.param {
            let catch_type = if ctx.options.use_unknown_in_catch_variables {
                ctx.arena.unknown()
            } else {
                ctx.arena.any()
            };
            bind_pattern(&param.pattern, catch_type, scoping, ctx);
        }
        check_block_statements(&handler.body.body, scoping, ctx);
        if !handler.body.body.last().is_some_and(statement_leaves_flow) {
            falling_out.push(ctx.narrow.clone());
        }
        let handler_span = handler.body.span();
        written_in_handler = ctx.writes.written_in(handler_span.start, handler_span.end);
    }

    // Both ended in `return`/`throw`: nothing after the statement is reachable, and the
    // state from before it is the least misleading one to keep.
    let mut after = join_all(falling_out, &mut ctx.arena).unwrap_or_else(|| outer.clone());

    if let Some(finalizer) = &try_stmt.finalizer {
        let mut at_finally = outer;
        for &symbol_id in written_in_try.iter().chain(&written_in_handler) {
            at_finally.remove(symbol_id);
        }
        ctx.narrow = at_finally;
        check_block_statements(&finalizer.body, scoping, ctx);
        let finalizer_span = finalizer.span();
        for symbol_id in ctx
            .writes
            .written_in(finalizer_span.start, finalizer_span.end)
        {
            after.remove(symbol_id);
        }
    }
    ctx.narrow = after;
}

pub(super) fn check_switch_statement(
    switch_stmt: &SwitchStatement,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) {
    infer_expression_type(&switch_stmt.discriminant, scoping, ctx);

    // `switch (kind)`, `switch (shape.kind)` and `switch (typeof x)` narrow the
    // discriminant's variable in each case body. Any other discriminant is
    // inferred normally and narrows nothing.
    let discriminant = switch_discriminant(&switch_stmt.discriminant, scoping);

    // Every case test, gathered once so default can exclude all of them: it has no
    // test of its own, only the complement of what the other cases claimed.
    let all_tests: Vec<&Expression> = switch_stmt
        .cases
        .iter()
        .filter_map(|case| case.test.as_ref())
        .collect();

    // Each case body starts from the narrowing the switch itself started with, not
    // from whatever the previous case left behind: cases are checked in textual
    // order regardless of fallthrough, so without this a guard clause in one case
    // would narrow the next case's code too, and every case's narrowing would
    // otherwise leak past the switch's closing brace.
    //
    // Case labels with an empty body fall into the next case (`case "a": case "b":
    // body`), so the body is narrowed to any of the labels it is reached through,
    // not just the last. Real fallthrough out of a non-empty body is not modeled:
    // each such case is narrowed as if reached directly, matching tsc for the
    // usual break/return style (see statement_always_exits).
    let outer_narrow = ctx.narrow.clone();
    // A `break` in a case leaves the switch, so it must not be taken for one that leaves
    // an enclosing loop; the frame only exists to catch it, and what it collects is dropped
    // because the state after the switch is, as before, the one from before it.
    ctx.jump_frames
        .push(JumpFrame::new(None, FrameKind::Switch));
    let mut pending_tests: Vec<&Expression> = Vec::new();
    let mut pending_default = false;
    for case in &switch_stmt.cases {
        ctx.narrow = outer_narrow.clone();
        match &case.test {
            Some(test) => {
                infer_expression_type(test, scoping, ctx);
                pending_tests.push(test);
            }
            None => pending_default = true,
        }
        if case.consequent.is_empty() {
            continue;
        }

        if let Some(discriminant) = &discriminant {
            // A default grouped with other labels can be reached with any value,
            // so only a lone default or a set of plain case labels narrows.
            let overlay = match (pending_default, pending_tests.is_empty()) {
                (false, false) => narrow_switch_case(discriminant, &pending_tests, ctx),
                (true, true) => narrow_switch_default(discriminant, &all_tests, ctx),
                _ => NarrowState::new(),
            };
            ctx.narrow.extend(overlay);
        }
        pending_tests.clear();
        pending_default = false;

        for stmt in &case.consequent {
            check_statement(stmt, scoping, ctx);
        }
    }
    ctx.jump_frames.pop();
    ctx.narrow = outer_narrow;
}
