// The backward flow resolver (F2), running beside the forward narrowing overlay and changing
// nothing.
//
// The overlay in `narrow.rs` threads a "what is known about each variable" state forward through
// the statements. The flow table (`crate::flow`) is the same program as a graph, and the question
// "what is the type of this read" is answered on it by walking antecedents backwards from the
// read: an assignment gives the assigned type, a condition narrows the type before it, a join
// unions its paths, a loop head iterates to a fixpoint. This module does that walk for every read
// of a variable and compares the answer with what the overlay produced at the same read, so every
// place the two disagree is a bug in one of them, found before the overlay is retired.
//
// It is off unless asked for (`enable_for_current_thread`, or `TSOX_FLOW_SHADOW=1` in the
// environment), and when on it only adds warnings (TSR9201); no type the checker computes is
// taken from here.
//
// What is reused rather than rewritten: a condition narrows a type by the overlay's own
// `narrow_condition` (a scratch overlay holding just the one variable at the type being
// narrowed), a `switch` clause by `narrow_switch_case` / `narrow_switch_default`, and an
// assignment by `narrow_on_assignment`'s rule, fed the assigned type the forward pass recorded at
// the assignment. So a disagreement is about the graph walk (which paths meet, in what order, what
// a loop settles to), not about how one test narrows one type. What it cannot answer it says so
// (`Unknown`) and the read is skipped: an `a ?? b` edge, a variable the checker has no type for.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};

use oxc_ast::AstKind;
use oxc_ast::ast::{Expression, Program, SwitchStatement};
use oxc_ast_visit::{Visit, walk};
use oxc_semantic::{Scoping, Semantic, SymbolId};
use oxc_span::{GetSpan, Span};

use super::context::CheckContext;
use super::narrow::{
    NarrowState, SwitchDiscriminant, assigned_narrowed_type, narrow_condition, narrow_switch_case,
    narrow_switch_default, switch_discriminant,
};
use crate::arena::TypeId;
use crate::flow::{Assume, EdgeKind, FileFlow, FlowId, FlowKind, build_with_roots};

thread_local! {
    static ENABLED: Cell<Option<bool>> = const { Cell::new(None) };
    static EVERY_READ: Cell<bool> = const { Cell::new(false) };
}

/// Turns the shadow resolver on or off for checks run on this thread. Tests use this; a process
/// that is not a test uses `TSOX_FLOW_SHADOW`.
pub fn enable_for_current_thread(on: bool) {
    ENABLED.with(|cell| cell.set(Some(on)));
}

/// With the shadow on for this thread, also report every read it could answer, agreeing or not
/// (TSR9202), so a test can see that the walk actually ran.
pub fn report_every_read_for_current_thread(on: bool) {
    EVERY_READ.with(|cell| cell.set(on));
}

// `TSOX_FLOW_SHADOW=1` reports disagreements; `TSOX_FLOW_SHADOW=all` also reports every read the
// walk answered, for comparing both sides with tsc.
fn mode() -> Mode {
    if let Some(on) = ENABLED.with(|cell| cell.get()) {
        return match (on, EVERY_READ.with(|cell| cell.get())) {
            (false, _) => Mode::Off,
            (true, false) => Mode::Disagreements,
            (true, true) => Mode::All,
        };
    }
    match std::env::var("TSOX_FLOW_SHADOW").as_deref() {
        Ok("all") => Mode::All,
        Ok("0") | Ok("") | Err(_) => Mode::Off,
        Ok(_) => Mode::Disagreements,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Off,
    Disagreements,
    All,
}

/// What the walk found for one question.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Found {
    /// No path reaches here (yet): the identity of a join.
    Bottom,
    Ty(TypeId),
    /// The walk met something it does not model.
    Unknown,
}

impl Found {
    fn join(self, other: Found, ctx: &mut CheckContext<'_, '_>) -> Found {
        match (self, other) {
            (Found::Unknown, _) | (_, Found::Unknown) => Found::Unknown,
            (Found::Bottom, other) | (other, Found::Bottom) => other,
            (Found::Ty(a), Found::Ty(b)) if a == b => Found::Ty(a),
            (Found::Ty(a), Found::Ty(b)) => Found::Ty(ctx.arena.alloc_union(vec![a, b])),
        }
    }
}

/// One read the walk answered, and what each side said.
#[derive(Clone, Debug)]
pub struct Observation {
    pub span: Span,
    pub name: String,
    pub overlay: String,
    pub flow: String,
    pub agree: bool,
}

#[derive(Default, Debug, Clone)]
pub struct ShadowStats {
    pub agreed: u32,
    pub disagreed: u32,
    pub unknown: u32,
    pub unreachable: u32,
}

type Key = (usize, FlowId, u32);

pub struct FlowShadow<'ast> {
    program: &'ast Program<'ast>,
    mode: Mode,
    graph: FileFlow,
    // Where each body's `ExprRef`s count from: add one to a reference to get a node index.
    roots: Vec<u32>,
    // A read's node index to the body it is in, the flow node it is read at, and its variable.
    reads: HashMap<u32, (usize, FlowId, u32)>,
    // Node indexes of variable declarators, whose assignment needs no record to be understood.
    declarators: HashSet<u32>,
    // Where the expressions the flow nodes name are, by node index; they are found again in the
    // program by span, since the visitor only lends an expression for the length of its callback.
    spans: HashMap<u32, Span>,
    // What the forward pass assigned at each assignment or declarator, by node index: the
    // variable's declared type and the assigned type.
    assigned: HashMap<u32, (TypeId, TypeId)>,
    // Results that do not depend on a loop assumption.
    cache: HashMap<Key, Found>,
    // Results computed while a loop head's type is only assumed; thrown away when it changes.
    scratch: HashMap<Key, Found>,
    assumptions: HashMap<Key, Found>,
    loop_depth: u32,
    recursion: u32,
    // The reads met so far, answered once the whole file has been checked: a read at the top of
    // a loop is met before the assignments in the loop's body, whose types the walk needs.
    pending: Vec<PendingRead>,
    observations: Vec<Observation>,
    pub stats: ShadowStats,
}

struct PendingRead {
    node_index: u32,
    symbol: SymbolId,
    name: String,
    span: Span,
    overlay: TypeId,
}

impl<'ast> FlowShadow<'ast> {
    /// Builds the shadow for a file, or `None` when it is off or the file has no flow to walk.
    pub fn new(program: &'ast Program<'ast>, semantic: &Semantic<'ast>) -> Option<Self> {
        let mode = mode();
        if mode == Mode::Off {
            return None;
        }
        let (graph, roots) = build_with_roots(semantic, program);
        if graph.bodies.is_empty() {
            return None;
        }
        let roots: Vec<u32> = roots.iter().map(|root| root.index() as u32).collect();
        let mut reads = HashMap::new();
        for (body_index, body) in graph.bodies.iter().enumerate() {
            let root = roots[body_index];
            for reference in &body.references {
                reads.insert(
                    root + reference.node.0,
                    (body_index, reference.flow, reference.symbol),
                );
            }
        }
        let mut needed: HashSet<u32> = HashSet::new();
        for (body_index, body) in graph.bodies.iter().enumerate() {
            let root = roots[body_index];
            for node in &body.nodes {
                match &node.kind {
                    FlowKind::Condition { test, .. } => {
                        needed.insert(root + test.0);
                    }
                    FlowKind::Clause {
                        discriminant,
                        matched,
                        all_tests,
                        ..
                    } => {
                        needed.insert(root + discriminant.0);
                        needed.extend(matched.iter().map(|e| root + e.0));
                        needed.extend(all_tests.iter().map(|e| root + e.0));
                    }
                    _ => {}
                }
            }
        }
        let mut declarators = HashSet::new();
        let mut spans = HashMap::new();
        for node in semantic.nodes().iter() {
            let index = node.id().index() as u32;
            if matches!(node.kind(), AstKind::VariableDeclarator(_)) {
                declarators.insert(index);
            }
            if needed.contains(&index) {
                spans.insert(index, node.kind().span());
            }
        }
        Some(Self {
            program,
            mode,
            graph,
            roots,
            reads,
            declarators,
            spans,
            assigned: HashMap::new(),
            cache: HashMap::new(),
            scratch: HashMap::new(),
            assumptions: HashMap::new(),
            loop_depth: 0,
            recursion: 0,
            pending: Vec::new(),
            observations: Vec::new(),
            stats: ShadowStats::default(),
        })
    }

    /// The forward pass assigned `assigned` to a variable declared as `declared` at the
    /// assignment or declarator with this node index.
    pub fn note_assignment(&mut self, node_index: u32, declared: TypeId, assigned: TypeId) {
        self.assigned.insert(node_index, (declared, assigned));
    }

    /// Notes a read and the type the overlay gave it. `overlay` is the type the checker is about
    /// to use for the read.
    pub fn note_read(
        &mut self,
        node_index: u32,
        symbol: SymbolId,
        name: &str,
        span: Span,
        overlay: TypeId,
    ) {
        if self.reads.contains_key(&node_index) {
            self.pending.push(PendingRead {
                node_index,
                symbol,
                name: name.to_string(),
                span,
                overlay,
            });
        }
    }

    // Compares the walk's answer for one read with what the overlay gave.
    fn resolve_read(
        &mut self,
        ctx: &mut CheckContext<'ast, '_>,
        scoping: &Scoping,
        read: PendingRead,
    ) {
        let Some(&(body, flow, read_symbol)) = self.reads.get(&read.node_index) else {
            return;
        };
        if read_symbol as usize != read.symbol.index() {
            return;
        }
        let Some(declared) = ctx.symbols.get(read.symbol) else {
            return;
        };
        let diagnostics_before = ctx.diagnostics.len();
        let found = self.type_at(
            ctx,
            scoping,
            body,
            flow,
            read.symbol.index() as u32,
            declared,
        );
        ctx.diagnostics.truncate(diagnostics_before);
        let walked = match found {
            Found::Unknown => {
                self.stats.unknown += 1;
                return;
            }
            Found::Bottom => {
                self.stats.unreachable += 1;
                return;
            }
            Found::Ty(ty) => ty,
        };
        let agree = ctx.arena.structurally_equal(walked, read.overlay);
        if agree {
            self.stats.agreed += 1;
        } else {
            self.stats.disagreed += 1;
        }
        if !agree || self.mode == Mode::All {
            self.observations.push(Observation {
                span: read.span,
                name: read.name,
                overlay: crate::type_display::display_type(&ctx.arena, read.overlay),
                flow: crate::type_display::display_type(&ctx.arena, walked),
                agree,
            });
        }
    }

    /// Emits what was found as warnings, in source order.
    pub fn report(mut self, ctx: &mut CheckContext<'ast, '_>, scoping: &Scoping) {
        for read in std::mem::take(&mut self.pending) {
            self.resolve_read(ctx, scoping, read);
        }
        if self.mode == Mode::All {
            eprintln!(
                "flow shadow: {} agreed, {} disagreed, {} unknown, {} unreachable",
                self.stats.agreed, self.stats.disagreed, self.stats.unknown, self.stats.unreachable
            );
        }
        self.observations.sort_by_key(|o| o.span.start);
        for observation in &self.observations {
            let message = if observation.agree {
                crate::diagnostic_messages::messages::flow_shadow_read(
                    &observation.name,
                    &observation.overlay,
                )
            } else {
                crate::diagnostic_messages::messages::flow_shadow_disagreement(
                    &observation.name,
                    &observation.overlay,
                    &observation.flow,
                )
            };
            ctx.warning(message, observation.span);
        }
    }

    // ---- the walk -------------------------------------------------------------------------

    fn type_at(
        &mut self,
        ctx: &mut CheckContext<'ast, '_>,
        scoping: &Scoping,
        body: usize,
        flow: FlowId,
        symbol: u32,
        declared: TypeId,
    ) -> Found {
        let key = (body, flow, symbol);
        if let Some(&found) = self.assumptions.get(&key) {
            return found;
        }
        if let Some(&found) = self.cache.get(&key).or_else(|| self.scratch.get(&key)) {
            return found;
        }
        if self.recursion > 3000 {
            return Found::Unknown;
        }
        self.recursion += 1;
        let found = self.compute(ctx, scoping, body, flow, symbol, declared);
        self.recursion -= 1;
        if self.loop_depth == 0 {
            self.cache.insert(key, found);
        } else {
            self.scratch.insert(key, found);
        }
        found
    }

    fn compute(
        &mut self,
        ctx: &mut CheckContext<'ast, '_>,
        scoping: &Scoping,
        body: usize,
        flow: FlowId,
        symbol: u32,
        declared: TypeId,
    ) -> Found {
        let node = self.graph.bodies[body].nodes[flow as usize].clone();
        let root = self.roots[body];
        match &node.kind {
            FlowKind::Start => Found::Ty(declared),
            FlowKind::LoopHead => self.loop_head(ctx, scoping, body, flow, symbol, declared),
            FlowKind::Join => {
                let mut found = Found::Bottom;
                for ante in &node.antecedents {
                    let before = self.type_at(ctx, scoping, body, ante.from, symbol, declared);
                    found = found.join(before, ctx);
                }
                found
            }
            FlowKind::Assign(assignment) => {
                let targets_this = matches!(
                    &assignment.target,
                    crate::flow::AssignTarget::Symbol { symbol: s, .. } if *s == symbol
                );
                if !targets_this {
                    return self.before(ctx, scoping, body, &node, symbol, declared);
                }
                let at = root + assignment.at.0;
                match self.assigned.get(&at).copied() {
                    Some((declared_here, assigned)) => {
                        Found::Ty(assigned_narrowed_type(ctx, declared_here, assigned))
                    }
                    // A declarator the forward pass did not narrow leaves the declared type; any
                    // other write it did not record (`x += 1`, a destructuring) is not modelled.
                    None if self.declarators.contains(&at) => Found::Ty(declared),
                    None => Found::Unknown,
                }
            }
            FlowKind::Condition { test, assume } => {
                let before = self.before(ctx, scoping, body, &node, symbol, declared);
                let Found::Ty(base) = before else {
                    return before;
                };
                let on_true = match assume {
                    Assume::True => true,
                    Assume::False => false,
                    // The branch of `a ?? b` taken on `a`'s being nullish or not: the overlay
                    // narrows a bare name only by truthiness, so there is nothing to compare.
                    Assume::Nullish | Assume::NotNullish => return Found::Unknown,
                };
                self.narrow_by_test(ctx, scoping, symbol, base, root + test.0, on_true)
            }
            FlowKind::Clause {
                discriminant,
                matched,
                includes_default,
                all_tests,
            } => {
                let before = self.before(ctx, scoping, body, &node, symbol, declared);
                let Found::Ty(base) = before else {
                    return before;
                };
                self.narrow_by_clause(
                    ctx,
                    scoping,
                    symbol,
                    base,
                    root + discriminant.0,
                    &matched.iter().map(|e| root + e.0).collect::<Vec<_>>(),
                    *includes_default,
                    &all_tests.iter().map(|e| root + e.0).collect::<Vec<_>>(),
                )
            }
        }
    }

    // The type on entry to a node that has one way in (or several, joined).
    fn before(
        &mut self,
        ctx: &mut CheckContext<'ast, '_>,
        scoping: &Scoping,
        body: usize,
        node: &crate::flow::FlowNode,
        symbol: u32,
        declared: TypeId,
    ) -> Found {
        let mut found = Found::Bottom;
        for ante in &node.antecedents {
            let before = self.type_at(ctx, scoping, body, ante.from, symbol, declared);
            found = found.join(before, ctx);
        }
        found
    }

    // A loop head's type is what it was on entry joined with what the loop gives back, and what
    // the loop gives back depends on the head's own type. Start from the entry type and iterate
    // until it stops changing.
    fn loop_head(
        &mut self,
        ctx: &mut CheckContext<'ast, '_>,
        scoping: &Scoping,
        body: usize,
        flow: FlowId,
        symbol: u32,
        declared: TypeId,
    ) -> Found {
        let key = (body, flow, symbol);
        let node = self.graph.bodies[body].nodes[flow as usize].clone();
        let mut entry = Found::Bottom;
        for ante in node.antecedents.iter().filter(|a| a.edge != EdgeKind::Back) {
            let before = self.type_at(ctx, scoping, body, ante.from, symbol, declared);
            entry = entry.join(before, ctx);
        }
        if entry == Found::Unknown {
            return Found::Unknown;
        }
        let mut current = entry;
        self.loop_depth += 1;
        for _ in 0..8 {
            self.assumptions.insert(key, current);
            self.scratch.clear();
            let mut back = Found::Bottom;
            for ante in node.antecedents.iter().filter(|a| a.edge == EdgeKind::Back) {
                let before = self.type_at(ctx, scoping, body, ante.from, symbol, declared);
                back = back.join(before, ctx);
            }
            let next = entry.join(back, ctx);
            if next == current || next == Found::Unknown {
                current = next;
                break;
            }
            current = next;
        }
        self.assumptions.remove(&key);
        self.loop_depth -= 1;
        self.scratch.clear();
        current
    }

    // Narrows `base`, the type of `symbol` before a test, by the test being true or false, with
    // the overlay's own rules: a scratch overlay holds just that variable at that type.
    fn narrow_by_test(
        &mut self,
        ctx: &mut CheckContext<'ast, '_>,
        scoping: &Scoping,
        symbol: u32,
        base: TypeId,
        test_index: u32,
        on_true: bool,
    ) -> Found {
        let symbol_id = SymbolId::new(symbol as usize);
        let mut result = Found::Unknown;
        let program = self.program;
        let Some(&span) = self.spans.get(&test_index) else {
            return Found::Unknown;
        };
        find_expression(program, span, &mut |test: &Expression| {
            let saved = std::mem::replace(&mut ctx.narrow, NarrowState::single(symbol_id, base));
            let (when_true, when_false) = narrow_condition(test, scoping, ctx);
            ctx.narrow = saved;
            let state = if on_true { when_true } else { when_false };
            result = Found::Ty(state.get(symbol_id).unwrap_or(base));
        });
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn narrow_by_clause(
        &mut self,
        ctx: &mut CheckContext<'ast, '_>,
        scoping: &Scoping,
        symbol: u32,
        base: TypeId,
        discriminant_index: u32,
        matched: &[u32],
        includes_default: bool,
        all_tests: &[u32],
    ) -> Found {
        let symbol_id = SymbolId::new(symbol as usize);
        let mut result = Found::Unknown;
        let program = self.program;
        let Some(&discriminant_span) = self.spans.get(&discriminant_index) else {
            return Found::Unknown;
        };
        let spans_of = |indexes: &[u32]| -> Vec<Span> {
            indexes
                .iter()
                .filter_map(|index| self.spans.get(index).copied())
                .collect()
        };
        let matched_spans = spans_of(matched);
        let all_spans = spans_of(all_tests);
        find_switch(
            program,
            discriminant_span,
            &mut |switch: &SwitchStatement| {
                let Some(discriminant) = switch_discriminant(&switch.discriminant, scoping) else {
                    return;
                };
                // The clause names the symbol it narrows only through its discriminant.
                let names_symbol = match &discriminant {
                    // The walk holds one variable at a time; `switch (true)` narrows through any.
                    SwitchDiscriminant::True => return,
                    SwitchDiscriminant::Identifier(s)
                    | SwitchDiscriminant::Property(s, _)
                    | SwitchDiscriminant::Typeof(s) => *s == symbol_id,
                };
                if !names_symbol {
                    result = Found::Ty(base);
                    return;
                }
                let tests_of = |spans: &[Span]| -> Vec<&Expression> {
                    switch
                        .cases
                        .iter()
                        .filter_map(|case| case.test.as_ref())
                        .filter(|test| spans.contains(&test.span()))
                        .collect()
                };
                let saved =
                    std::mem::replace(&mut ctx.narrow, NarrowState::single(symbol_id, base));
                let state = if includes_default {
                    narrow_switch_default(&discriminant, &tests_of(&all_spans), scoping, ctx)
                } else {
                    narrow_switch_case(&discriminant, &tests_of(&matched_spans), scoping, ctx)
                };
                ctx.narrow = saved;
                result = Found::Ty(state.get(symbol_id).unwrap_or(base));
            },
        );
        result
    }
}

// Finds the expression with this node index and lends it to `found`: the visitor only hands a
// reference out for the length of its callback, so whatever uses the expression has to run in it.
fn find_expression<'ast>(
    program: &'ast Program<'ast>,
    span: Span,
    found: &mut dyn FnMut(&Expression),
) {
    struct Find<'f> {
        target: Span,
        found: &'f mut dyn FnMut(&Expression),
        done: bool,
    }
    impl<'a, 'f> Visit<'a> for Find<'f> {
        fn visit_expression(&mut self, expression: &Expression<'a>) {
            if self.done {
                return;
            }
            if expression.span() == self.target {
                self.done = true;
                (self.found)(expression);
                return;
            }
            walk::walk_expression(self, expression);
        }
    }
    let mut find = Find {
        target: span,
        found,
        done: false,
    };
    find.visit_program(program);
}

// The `switch` whose discriminant has this node index.
fn find_switch<'ast>(
    program: &'ast Program<'ast>,
    discriminant: Span,
    found: &mut dyn FnMut(&SwitchStatement),
) {
    struct Find<'f> {
        target: Span,
        found: &'f mut dyn FnMut(&SwitchStatement),
        done: bool,
    }
    impl<'a, 'f> Visit<'a> for Find<'f> {
        fn visit_switch_statement(&mut self, switch: &SwitchStatement<'a>) {
            if self.done {
                return;
            }
            if switch.discriminant.span() == self.target {
                self.done = true;
                (self.found)(switch);
                return;
            }
            walk::walk_switch_statement(self, switch);
        }
    }
    let mut find = Find {
        target: discriminant,
        found,
        done: false,
    };
    find.visit_program(program);
}
