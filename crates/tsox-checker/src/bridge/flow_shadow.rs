// The backward flow resolver (F2).
//
// The overlay in `narrow.rs` threads a "what is known about each variable" state forward through
// the statements. The flow table (`crate::flow`) is the same program as a graph, and the question
// "what is the type of this read" is answered on it by walking antecedents backwards from the
// read: an assignment gives the assigned type, a condition narrows the type before it, a join
// unions its paths, a loop head iterates to a fixpoint. This module does that walk, and it has two
// roles.
//
// Active (the default): the walk answers each read of a variable whose declared type narrowing
// changes (a union, or `unknown`), and the overlay answers whenever the walk cannot. The walk
// says it cannot, and the overlay keeps the read, for:
//   * code that can be reached through an exception edge (`try`/`catch`/`finally`): the graph
//     enters a `catch` from the end of its `try`, where tsc enters it from the start and from after
//     every assignment, so the overlay stays in charge there until the graph is fixed;
//   * an `a ?? b` edge, and an assignment whose type the forward pass has not recorded: an
//     `x += 1`, a destructuring, or a write later in a loop's body than the read at its top;
//   * a variable declared in an outer function (the overlay carries narrowing of a `const` into a
//     closure; the walk starts a function from the declared type);
//   * a variable in a destructuring group, and every variable in a file that declares an
//     assertion function: the overlay narrows through those at places the graph has no node for;
//   * a read nothing reaches.
// `TSOX_FLOW=overlay` turns the walk off and leaves the overlay alone.
//
// Shadow (`TSOX_FLOW_SHADOW=1`): the overlay answers and the walk is compared with it at every read,
// each difference reported as a warning (TSR9201). `=all` also reports every read the walk answered
// (TSR9202), for joining both sides with tsc's types. This is how the cutover was measured.
//
// What is reused rather than rewritten: a condition narrows a type by the overlay's own
// `narrow_condition` (a scratch overlay holding just the one variable at the type being
// narrowed), a `switch` clause by `narrow_switch_case` / `narrow_switch_default`, and an
// assignment by `assigned_narrowed_type`, fed the assigned type the forward pass recorded at the
// assignment. So a disagreement is about the graph walk (which paths meet, in what order, what a
// loop settles to), not about how one test narrows one type.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};

use oxc_ast::AstKind;
use oxc_ast::ast::{Expression, Program, SwitchStatement};
use oxc_semantic::{Scoping, Semantic, SymbolId};
use oxc_span::{GetSpan, Span};

use super::context::CheckContext;
use super::narrow::{
    NarrowState, SwitchDiscriminant, assigned_narrowed_type, narrow_condition, narrow_switch_case,
    narrow_switch_default, switch_discriminant,
};
use crate::arena::TypeId;
use crate::flow::{Assume, EdgeKind, FileFlow, FlowId, FlowKind, build_with_roots};
use crate::types::Type;

thread_local! {
    static ENABLED: Cell<Option<bool>> = const { Cell::new(None) };
    static EVERY_READ: Cell<bool> = const { Cell::new(false) };
    static OVERLAY_ONLY: Cell<bool> = const { Cell::new(false) };
}

/// Turns the comparison (shadow mode) on or off for checks run on this thread. Tests use this; a
/// process that is not a test uses `TSOX_FLOW_SHADOW`.
pub fn enable_for_current_thread(on: bool) {
    ENABLED.with(|cell| cell.set(Some(on)));
}

/// With the shadow on for this thread, also report every read it could answer, agreeing or not
/// (TSR9202), so a test can see that the walk actually ran.
pub fn report_every_read_for_current_thread(on: bool) {
    EVERY_READ.with(|cell| cell.set(on));
}

/// Makes checks on this thread use the forward overlay alone, as if `TSOX_FLOW=overlay`.
pub fn use_overlay_only_for_current_thread(on: bool) {
    OVERLAY_ONLY.with(|cell| cell.set(on));
}

// Which role the walk plays. A thread-local setting wins over the environment: shadow comparison
// first, then overlay-only, then the default, which is for the walk to answer.
fn mode() -> Mode {
    if let Some(on) = ENABLED.with(|cell| cell.get()) {
        return match (on, EVERY_READ.with(|cell| cell.get())) {
            (false, _) => Mode::Off,
            (true, false) => Mode::Disagreements,
            (true, true) => Mode::All,
        };
    }
    if OVERLAY_ONLY.with(|cell| cell.get()) {
        return Mode::Off;
    }
    match std::env::var("TSOX_FLOW_SHADOW").as_deref() {
        Ok("all") => return Mode::All,
        Ok("0") | Ok("") | Err(_) => {}
        Ok(_) => return Mode::Disagreements,
    }
    match std::env::var("TSOX_FLOW").as_deref() {
        Ok("overlay") => Mode::Off,
        _ => Mode::Active,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// The walk plays no part.
    Off,
    /// The walk answers where it can; the overlay answers where it cannot.
    Active,
    Disagreements,
    All,
}

// The kinds of structure a body can have that the walk may be switched on for separately. A body
// with neither is conditions and assignments only.
const SWITCH: u8 = 1;
const LOOP: u8 = 2;

// Which of them the walk answers for: `TSOX_FLOW_FEATURES`, a comma-separated list of `switch` and
// `loop`, with `all` (the default) and `none`.
fn enabled_features() -> u8 {
    let value = std::env::var("TSOX_FLOW_FEATURES").unwrap_or_else(|_| "all".to_string());
    value.split(',').fold(0, |features, name| {
        features
            | match name.trim() {
                "all" => SWITCH | LOOP,
                "switch" => SWITCH,
                "loop" => LOOP,
                _ => 0,
            }
    })
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
    mode: Mode,
    graph: FileFlow,
    // Where each body's `ExprRef`s count from: add one to a reference to get a node index.
    roots: Vec<u32>,
    // A read's node index to the body it is in, the flow node it is read at, and its variable.
    reads: HashMap<u32, (usize, FlowId, u32)>,
    // Node indexes of variable declarators, whose assignment needs no record to be understood.
    declarators: HashSet<u32>,
    // The statement each read sits in (by where it starts), to tell a read in dead code, and which
    // kinds of structure each body has, so each kind can be switched on separately.
    read_statement: HashMap<u32, u32>,
    body_uses: Vec<u8>,
    features: u8,
    // Where the expressions the flow nodes name are, by node index, and the expressions
    // themselves where they can be taken from the node that holds them: a test from its `if`, a
    // `switch`'s discriminant from the `switch`. Holding the semantic nodes' own references keeps
    // the lookup constant time and the lifetimes honest.
    spans: HashMap<u32, Span>,
    expressions: HashMap<u32, &'ast Expression<'ast>>,
    switches: HashMap<u32, &'ast SwitchStatement<'ast>>,
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
        let body_uses: Vec<u8> = graph
            .bodies
            .iter()
            .map(|body| {
                body.nodes.iter().fold(0u8, |uses, node| match node.kind {
                    FlowKind::Clause { .. } => uses | SWITCH,
                    FlowKind::LoopHead => uses | LOOP,
                    _ => uses,
                })
            })
            .collect();
        let mut read_statement = HashMap::new();
        let mut declarators = HashSet::new();
        let mut spans = HashMap::new();
        let mut expressions = HashMap::new();
        let mut switches = HashMap::new();
        for node in semantic.nodes().iter() {
            let index = node.id().index() as u32;
            if matches!(node.kind(), AstKind::VariableDeclarator(_)) {
                declarators.insert(index);
            }
            if reads.contains_key(&index) {
                // The nearest statement around the read, from the node's ancestors.
                if let Some(start) = semantic
                    .nodes()
                    .ancestor_kinds(node.id())
                    .find_map(statement_start)
                {
                    read_statement.insert(index, start);
                }
            }
            if needed.contains(&index) {
                let span = node.kind().span();
                spans.insert(index, span);
                let parent = semantic.nodes().parent_kind(node.id());
                if let Some(expression) = expression_child(parent, span) {
                    expressions.insert(index, expression);
                }
                if let AstKind::SwitchStatement(switch) = parent {
                    if switch.discriminant.span() == span {
                        switches.insert(index, switch);
                    }
                }
            }
        }
        let _ = program;
        Some(Self {
            mode,
            graph,
            roots,
            reads,
            declarators,
            read_statement,
            body_uses,
            features: enabled_features(),
            spans,
            expressions,
            switches,
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

    /// A read of a variable, with the type the overlay gave it. In Active mode this answers with
    /// the walk's type when it has one; otherwise it notes the read for the comparison.
    pub fn on_read(
        &mut self,
        ctx: &mut CheckContext<'ast, '_>,
        scoping: &Scoping,
        node_index: u32,
        symbol: SymbolId,
        name: &str,
        span: Span,
        overlay: TypeId,
    ) -> Option<TypeId> {
        if self.mode == Mode::Active {
            return self.resolve_now(ctx, scoping, node_index, symbol);
        }
        self.note_read(node_index, symbol, name, span, overlay);
        None
    }

    // The walk's type for a read, now, or `None` when the overlay should answer. Only a declared
    // type that narrowing changes is asked about; anything else reads as itself either way. What
    // earlier reads found is reused (see `type_at`).
    fn resolve_now(
        &mut self,
        ctx: &mut CheckContext<'ast, '_>,
        scoping: &Scoping,
        node_index: u32,
        symbol: SymbolId,
    ) -> Option<TypeId> {
        let &(body, flow, read_symbol) = self.reads.get(&node_index)?;
        if read_symbol as usize != symbol.index() {
            return None;
        }
        // Two ways the overlay narrows that the graph has no node for. A test on one binding of a
        // destructuring narrows the others (`const { kind, value } = x`), which a walk that holds
        // one variable at a time cannot see; and a call to a function that `asserts` narrows its
        // argument from the statement on, and a call is not a node of the graph. So a variable in a
        // destructuring group, and every variable in a file that declares an assertion function,
        // stays with the overlay.
        if ctx.destructured_index.contains_key(&symbol)
            || ctx
                .type_predicates
                .values()
                .any(|predicate| predicate.asserts)
        {
            return None;
        }
        // A body with a kind of structure the walk is not switched on for stays with the overlay.
        if self.body_uses[body] & !self.features != 0 {
            return None;
        }
        // A read in code that cannot run: tsc gives it the declared type, which the overlay does.
        if self
            .read_statement
            .get(&node_index)
            .is_some_and(|start| ctx.flow.is_dead(*start))
        {
            return None;
        }
        let declared = ctx.symbols.get(symbol)?;
        if !matches!(ctx.arena.get(declared), Type::Union(_) | Type::Unknown) {
            return None;
        }
        self.scratch.clear();
        let diagnostics_before = ctx.diagnostics.len();
        let found = self.type_at(ctx, scoping, body, flow, symbol.index() as u32, declared);
        ctx.diagnostics.truncate(diagnostics_before);
        match found {
            Found::Ty(ty) => Some(ty),
            _ => None,
        }
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
        // What the walk found is kept across reads, except `Unknown`: an assignment the forward
        // pass has not reached yet (later in a loop's body) turns an unknown into a known type, so
        // it is asked again, while a type or an unreachable result can only be the same later.
        if found != Found::Unknown {
            if self.loop_depth == 0 {
                self.cache.insert(key, found);
            } else {
                self.scratch.insert(key, found);
            }
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
        // A node entered through an exception edge: the graph's catch entry is incomplete (see the
        // header), so the overlay answers for everything the edge reaches.
        if self.mode == Mode::Active
            && node
                .antecedents
                .iter()
                .any(|a| a.edge == EdgeKind::Exception)
        {
            return Found::Unknown;
        }
        match &node.kind {
            FlowKind::Start => {
                // The top of a function. A variable of an outer function is the overlay's to
                // answer: it carries what is known of a `const` into a closure, and this starts
                // from the declared type.
                let declaring = scoping
                    .symbol_declaration(SymbolId::new(symbol as usize))
                    .index() as u32;
                if self.mode == Mode::Active && declaring <= root {
                    Found::Unknown
                } else {
                    Found::Ty(declared)
                }
            }
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
                // A bare `true` or `false` is a constant condition: the edge that contradicts it
                // is never taken, as in tsc's binder, and nothing flows along it. The graph
                // builder does not know that, so `while (true)` has an exit by its condition that
                // does not exist.
                if let Some(Expression::BooleanLiteral(literal)) =
                    self.expressions.get(&(root + test.0)).copied()
                {
                    let contradicted = match assume {
                        Assume::True => !literal.value,
                        Assume::False => literal.value,
                        Assume::Nullish | Assume::NotNullish => false,
                    };
                    if contradicted {
                        return Found::Bottom;
                    }
                }
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
        let Some(&test) = self.expressions.get(&test_index) else {
            return Found::Unknown;
        };
        let saved = std::mem::replace(&mut ctx.narrow, NarrowState::single(symbol_id, base));
        let (when_true, when_false) = narrow_condition(test, scoping, ctx);
        ctx.narrow = saved;
        let state = if on_true { when_true } else { when_false };
        Found::Ty(state.get(symbol_id).unwrap_or(base))
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
        let Some(&switch) = self.switches.get(&discriminant_index) else {
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
        let Some(discriminant) = switch_discriminant(&switch.discriminant, scoping) else {
            return Found::Unknown;
        };
        // The clause names the symbol it narrows only through its discriminant.
        let names_symbol = match &discriminant {
            // The walk holds one variable at a time; `switch (true)` narrows through any.
            SwitchDiscriminant::True => return Found::Unknown,
            SwitchDiscriminant::Identifier(s)
            | SwitchDiscriminant::Property(s, _)
            | SwitchDiscriminant::Typeof(s) => *s == symbol_id,
        };
        if !names_symbol {
            return Found::Ty(base);
        }
        let tests_of = |spans: &[Span]| -> Vec<&Expression> {
            switch
                .cases
                .iter()
                .filter_map(|case| case.test.as_ref())
                .filter(|test| spans.contains(&test.span()))
                .collect()
        };
        let saved = std::mem::replace(&mut ctx.narrow, NarrowState::single(symbol_id, base));
        let state = if includes_default {
            narrow_switch_default(&discriminant, &tests_of(&all_spans), scoping, ctx)
        } else {
            narrow_switch_case(&discriminant, &tests_of(&matched_spans), scoping, ctx)
        };
        ctx.narrow = saved;
        Found::Ty(state.get(symbol_id).unwrap_or(base))
    }
}

// Where a statement starts, for the kinds that can hold a read.
fn statement_start(kind: AstKind<'_>) -> Option<u32> {
    match kind {
        AstKind::ExpressionStatement(it) => Some(it.span.start),
        AstKind::IfStatement(it) => Some(it.span.start),
        AstKind::WhileStatement(it) => Some(it.span.start),
        AstKind::DoWhileStatement(it) => Some(it.span.start),
        AstKind::ForStatement(it) => Some(it.span.start),
        AstKind::ForInStatement(it) => Some(it.span.start),
        AstKind::ForOfStatement(it) => Some(it.span.start),
        AstKind::ReturnStatement(it) => Some(it.span.start),
        AstKind::ThrowStatement(it) => Some(it.span.start),
        AstKind::SwitchStatement(it) => Some(it.span.start),
        AstKind::TryStatement(it) => Some(it.span.start),
        AstKind::LabeledStatement(it) => Some(it.span.start),
        AstKind::VariableDeclaration(it) => Some(it.span.start),
        _ => None,
    }
}

// The expression among a node's own fields that has this span: the test of an `if`, a loop or a
// conditional, an operand of `&&`, `||` or `??`, the argument of `!`, the discriminant or test of a
// `switch`. These are the places a condition the flow graph names can sit.
fn expression_child<'a>(parent: AstKind<'a>, span: Span) -> Option<&'a Expression<'a>> {
    let pick = |expression: &'a Expression<'a>| (expression.span() == span).then_some(expression);
    match parent {
        AstKind::IfStatement(it) => pick(&it.test),
        AstKind::WhileStatement(it) => pick(&it.test),
        AstKind::DoWhileStatement(it) => pick(&it.test),
        AstKind::ForStatement(it) => it.test.as_ref().and_then(pick),
        AstKind::ConditionalExpression(it) => pick(&it.test),
        AstKind::LogicalExpression(it) => pick(&it.left).or_else(|| pick(&it.right)),
        AstKind::UnaryExpression(it) => pick(&it.argument),
        AstKind::ParenthesizedExpression(it) => pick(&it.expression),
        AstKind::ExpressionStatement(it) => pick(&it.expression),
        AstKind::SwitchStatement(it) => pick(&it.discriminant),
        AstKind::SwitchCase(it) => it.test.as_ref().and_then(pick),
        _ => None,
    }
}
