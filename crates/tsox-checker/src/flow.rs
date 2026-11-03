// The flow graph of a file: for every function body (and the top level), the sequence of
// events that decide what a name's type is at a point, as arena-free data.
//
// Problem: narrowing is done by a forward walk over the statements that carries an overlay
// of narrowed types (bridge::narrow). It is eager and depends on checking order, it
// approximates try/finally, labeled jumps and switch fallthrough, and it cannot answer "what
// is the type of this reference" without having walked everything before it. oxc already
// builds a control-flow graph, but it records only block structure: the edges are not
// labeled true or false, the right operand of `&&` has no instruction, and assignments and
// switch clauses are not marked.
// Picked: the hybrid in docs/architecture/flow.md. oxc_cfg supplies the skeleton: blocks,
// edges, loops (`Backedge`), try/catch/finally (`Finalize`, explicit `Error`), reachability,
// and the node-to-block map. One pass over the AST supplies what the skeleton lacks: which
// successor is the true or the false one, assignments, switch clauses. The result is a table
// of flow nodes in the shape tsc's binder produces (Start, Join, LoopHead, Assign,
// Condition, Clause), so a reference's type can later be found by walking antecedents
// backwards and lazily, one reference at a time.
// Cost: this is a table only; nothing reads it for narrowing yet (F2 of the plan runs the
// new resolver beside the old overlay before anything is replaced).
//
// Gaps, on purpose, until a phase needs them: optional chains do not split the graph;
// class property initializers and static blocks have no body of their own; a call is not
// an event (assertion functions and never-returning calls come with F3); a declaration
// without an initializer is not an event (definite assignment comes with F3).
//
// Identity: an expression is named by its position in the body, counted in AST order from
// the body's root node, never by byte offset, so a comment or blank line above a function
// does not change its graph. A symbol is named by its index in the file's symbol table plus
// its name; adding a declaration earlier in the file renumbers them, so a consumer that
// needs a key that survives that (the query layer) derives it from the name.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use oxc_ast::{
    AstKind,
    ast::{AssignmentTarget, Expression, LogicalOperator, Program},
};
use oxc_cfg::{
    BlockNodeId, ControlFlowGraph, EdgeType, ErrorEdgeKind, InstructionKind,
    graph::{Direction, visit::EdgeRef},
};
use oxc_semantic::{NodeId, Semantic};
use oxc_span::{GetSpan, Span};

use crate::error::CheckerError;

pub type FlowId = u32;

/// An expression, as its position in the body: the AST-order index of its node minus that of
/// the body's root node.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ExprRef(pub u32);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FileFlow {
    pub bodies: Vec<BodyFlow>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BodyKind {
    Program,
    Function,
    Arrow,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BodyId {
    /// Position among the file's bodies, in AST order.
    pub ordinal: u32,
    pub kind: BodyKind,
    pub name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BodyFlow {
    pub id: BodyId,
    pub nodes: Vec<FlowNode>,
    pub entry: FlowId,
    /// Where control is when the body runs off its end without a `return`, if it can. A
    /// function whose last statement always returns or throws has none.
    pub implicit_return: Option<FlowId>,
    /// Every read of a name in this body, with the flow node it is read at. Reads inside
    /// a nested function belong to that function's body, not this one.
    pub references: Vec<ReferenceFlow>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ReferenceFlow {
    pub node: ExprRef,
    pub symbol: u32,
    pub flow: FlowId,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FlowNode {
    pub kind: FlowKind,
    pub antecedents: Vec<Antecedent>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Antecedent {
    pub from: FlowId,
    pub edge: EdgeKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EdgeKind {
    Normal,
    /// The edge that closes a loop.
    Back,
    /// Into a `finally` block.
    Finalize,
    /// Into a `catch` block, from the `try` block.
    Exception,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum FlowKind {
    Start,
    /// Where paths meet.
    Join,
    /// Where a loop starts; one of its antecedents has `EdgeKind::Back`.
    LoopHead,
    Assign(Assignment),
    /// The path was taken because `test` evaluated as `assume` says.
    Condition {
        test: ExprRef,
        assume: Assume,
    },
    /// The path enters a `case` body by dispatch (not by falling through from the clause
    /// above): `switch (discriminant)` matched one of `matched`, or, when `includes_default`,
    /// matched none of `all_tests`.
    Clause {
        discriminant: ExprRef,
        matched: Vec<ExprRef>,
        includes_default: bool,
        all_tests: Vec<ExprRef>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Assume {
    True,
    False,
    /// `a ?? b` went on to `b`: `a` was null or undefined.
    Nullish,
    /// `a ?? b` stopped at `a`.
    NotNullish,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Assignment {
    pub target: AssignTarget,
    /// The assigning expression or declarator.
    pub at: ExprRef,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum AssignTarget {
    Symbol {
        symbol: u32,
        name: String,
    },
    Member {
        symbol: u32,
        name: String,
        path: Vec<String>,
    },
}

impl FileFlow {
    /// A readable listing, one line per flow node, for tests and debugging.
    pub fn dump(&self) -> String {
        let mut out = String::new();
        for body in &self.bodies {
            out.push_str(&format!(
                "body #{} {:?} {:?}\n",
                body.id.ordinal, body.id.kind, body.id.name
            ));
            for (index, node) in body.nodes.iter().enumerate() {
                out.push_str(&format!("  f{index}: {:?} <-", node.kind));
                for ante in &node.antecedents {
                    out.push_str(&format!(" f{}[{:?}]", ante.from, ante.edge));
                }
                out.push('\n');
            }
        }
        out
    }
}

/// Parses `source` and builds its flow graph.
///
/// Runs on a thread with the checker's large stack: oxc recurses on nested syntax, and a
/// file such as a thousand-operand `||` chain would otherwise abort the process.
pub fn build_file_flow(source: &str, file_name: &str) -> Result<FileFlow, CheckerError> {
    crate::stack_guard::on_big_stack(|| {
        let allocator = oxc_allocator::Allocator::default();
        let program = crate::bridge::parse(&allocator, source, file_name)?;
        let semantic = crate::bridge::analyze(&program);
        Ok(build_flow(&semantic, &program))
    })
}

pub fn build_flow(semantic: &Semantic<'_>, program: &Program<'_>) -> FileFlow {
    build_with_roots(semantic, program).0
}

/// The graph, and the root node of each body (what its `ExprRef`s count from).
pub(crate) fn build_with_roots(
    semantic: &Semantic<'_>,
    _program: &Program<'_>,
) -> (FileFlow, Vec<NodeId>) {
    let Some(cfg) = semantic.cfg() else {
        return (FileFlow { bodies: Vec::new() }, Vec::new());
    };
    let mut builder = Builder::new(semantic, cfg);
    builder.collect_annotations();
    builder.collect_switches();
    let mut bodies = Vec::new();
    let mut roots = Vec::new();
    for (ordinal, (root, kind, name)) in builder.bodies().into_iter().enumerate() {
        let id = BodyId {
            ordinal: ordinal as u32,
            kind,
            name,
        };
        bodies.push(builder.build_body(id, root));
        roots.push(root);
    }
    (FileFlow { bodies }, roots)
}

type Block = BlockNodeId;

struct Cond {
    test: NodeId,
    assume: Assume,
}

struct Switch {
    discriminant: NodeId,
    discriminant_span: Span,
    head: Block,
    /// The block of each case, in source order: where its test is evaluated (the default's
    /// is an empty marker block).
    test_blocks: Vec<Block>,
    /// The test of each case; `None` for `default`.
    tests: Vec<Option<NodeId>>,
    /// The block each case's dispatch edge leads to.
    bodies: Vec<Option<Block>>,
}

struct Brancher {
    span: Span,
    node: NodeId,
}

struct Builder<'s, 'a> {
    sem: &'s Semantic<'a>,
    cfg: &'s ControlFlowGraph,
    /// Conditions that hold on an edge, keyed by the edge's two blocks.
    conditions: HashMap<(Block, Block), Vec<Cond>>,
    switches: Vec<Switch>,
    branchers: Vec<Brancher>,
}

impl<'s, 'a> Builder<'s, 'a> {
    fn new(sem: &'s Semantic<'a>, cfg: &'s ControlFlowGraph) -> Self {
        let mut branchers = Vec::new();
        for node in sem.nodes().iter() {
            match node.kind() {
                AstKind::LogicalExpression(_) | AstKind::ConditionalExpression(_) => {
                    branchers.push(Brancher {
                        span: node.kind().span(),
                        node: node.id(),
                    });
                }
                _ => {}
            }
        }
        Self {
            sem,
            cfg,
            conditions: HashMap::new(),
            switches: Vec::new(),
            branchers,
        }
    }

    fn block_of(&self, node: NodeId) -> Block {
        self.sem.nodes().cfg_id(node)
    }

    fn live(&self, block: Block) -> bool {
        !self.cfg.basic_block(block).is_unreachable()
    }

    fn classify(edge: &EdgeType) -> Option<EdgeKind> {
        match edge {
            EdgeType::Normal | EdgeType::Jump | EdgeType::Join => Some(EdgeKind::Normal),
            EdgeType::Backedge => Some(EdgeKind::Back),
            EdgeType::Finalize => Some(EdgeKind::Finalize),
            EdgeType::Error(ErrorEdgeKind::Explicit) => Some(EdgeKind::Exception),
            EdgeType::Error(ErrorEdgeKind::Implicit)
            | EdgeType::NewFunction
            | EdgeType::Unreachable => None,
        }
    }

    /// The reachable successors of `block`, once each, in a fixed order.
    fn succs(&self, block: Block) -> Vec<(Block, EdgeKind)> {
        let mut out: Vec<(Block, EdgeKind)> = Vec::new();
        for edge in self.cfg.graph.edges_directed(block, Direction::Outgoing) {
            let Some(kind) = Self::classify(edge.weight()) else {
                continue;
            };
            let target = edge.target();
            if self.live(target) && !out.iter().any(|(b, _)| *b == target) {
                out.push((target, kind));
            }
        }
        out.sort_by_key(|(b, _)| b.index());
        out
    }

    fn preds(&self, block: Block) -> Vec<(Block, EdgeKind)> {
        let mut out: Vec<(Block, EdgeKind)> = Vec::new();
        for edge in self.cfg.graph.edges_directed(block, Direction::Incoming) {
            let Some(kind) = Self::classify(edge.weight()) else {
                continue;
            };
            let source = edge.source();
            if self.live(source) && !out.iter().any(|(b, _)| *b == source) {
                out.push((source, kind));
            }
        }
        out.sort_by_key(|(b, _)| b.index());
        out
    }

    // ---- where an expression ends -------------------------------------------------------

    /// The block control is in once the expression spanning `span` has been evaluated.
    /// Most expressions end where they start; one that contains `&&`, `||`, `??` or `?:`
    /// ends in the block where those paths meet, which is the successor of the last of them
    /// other than its right operand (or, for `?:`, the successor of an arm).
    fn end_block(&self, span: Span, fallback: Block, within: &BTreeSet<Block>) -> Block {
        let mut best: Option<&Brancher> = None;
        for brancher in &self.branchers {
            let s = brancher.span;
            if s.start < span.start || s.end > span.end {
                continue;
            }
            if !within.contains(&self.block_of(brancher.node)) {
                continue;
            }
            let better = match best {
                None => true,
                Some(current) => {
                    (s.end, s.end - s.start)
                        > (current.span.end, current.span.end - current.span.start)
                }
            };
            if better {
                best = Some(brancher);
            }
        }
        best.and_then(|b| self.join_of(b.node, within))
            .unwrap_or(fallback)
    }

    fn join_of(&self, node: NodeId, within: &BTreeSet<Block>) -> Option<Block> {
        // A chain of ten thousand `&&` recurses once per link; past the stack budget the
        // expression is taken to end where it starts.
        if crate::stack_guard::exhausted() {
            return None;
        }
        match self.sem.nodes().kind(node) {
            AstKind::LogicalExpression(logical) => {
                let start = self.block_of(logical.left.node_id());
                let left_end = self.end_block(logical.left.span(), start, within);
                let right = self.block_of(logical.right.node_id());
                let others: Vec<Block> = self
                    .succs(left_end)
                    .into_iter()
                    .filter(|(b, k)| *b != right && *k != EdgeKind::Exception)
                    .map(|(b, _)| b)
                    .collect();
                (others.len() == 1).then(|| others[0])
            }
            AstKind::ConditionalExpression(cond) => {
                for arm in [&cond.consequent, &cond.alternate] {
                    let start = self.block_of(arm.node_id());
                    let end = self.end_block(arm.span(), start, within);
                    let next: Vec<Block> = self
                        .succs(end)
                        .into_iter()
                        .filter(|(_, k)| *k == EdgeKind::Normal)
                        .map(|(b, _)| b)
                        .collect();
                    if next.len() == 1 {
                        return Some(next[0]);
                    }
                }
                None
            }
            _ => None,
        }
    }

    // ---- conditions on edges ------------------------------------------------------------

    /// `test` decides between `target` and the other successors of the block that branches
    /// to it: entering `target` means `assume_target`, entering any other means
    /// `assume_other`.
    fn annotate(
        &mut self,
        test: NodeId,
        target: Block,
        assume_target: Assume,
        assume_other: Assume,
    ) {
        for (pred, _) in self.preds(target) {
            if pred == target {
                continue;
            }
            let succs: Vec<Block> = self
                .succs(pred)
                .into_iter()
                .filter(|(_, k)| *k != EdgeKind::Exception)
                .map(|(b, _)| b)
                .collect();
            if succs.len() < 2 || !succs.contains(&target) {
                continue;
            }
            for next in succs {
                let assume = if next == target {
                    assume_target
                } else {
                    assume_other
                };
                self.conditions
                    .entry((pred, next))
                    .or_default()
                    .push(Cond { test, assume });
            }
        }
    }

    fn collect_annotations(&mut self) {
        let mut jobs: Vec<(NodeId, NodeId, Assume, Assume)> = Vec::new();
        for node in self.sem.nodes().iter() {
            match node.kind() {
                AstKind::IfStatement(s) => jobs.push((
                    s.test.node_id(),
                    s.consequent.node_id(),
                    Assume::True,
                    Assume::False,
                )),
                AstKind::ConditionalExpression(c) => jobs.push((
                    c.test.node_id(),
                    c.consequent.node_id(),
                    Assume::True,
                    Assume::False,
                )),
                AstKind::WhileStatement(s) => jobs.push((
                    s.test.node_id(),
                    s.body.node_id(),
                    Assume::True,
                    Assume::False,
                )),
                AstKind::DoWhileStatement(s) => jobs.push((
                    s.test.node_id(),
                    s.body.node_id(),
                    Assume::True,
                    Assume::False,
                )),
                AstKind::ForStatement(s) => {
                    if let Some(test) = &s.test {
                        jobs.push((
                            test.node_id(),
                            s.body.node_id(),
                            Assume::True,
                            Assume::False,
                        ));
                    }
                }
                AstKind::LogicalExpression(l) => {
                    let (to_right, to_other) = match l.operator {
                        LogicalOperator::And => (Assume::True, Assume::False),
                        LogicalOperator::Or => (Assume::False, Assume::True),
                        LogicalOperator::Coalesce => (Assume::Nullish, Assume::NotNullish),
                    };
                    jobs.push((l.left.node_id(), l.right.node_id(), to_right, to_other));
                }
                _ => {}
            }
        }
        for (test, target_node, to_target, to_other) in jobs {
            let target = self.block_of(target_node);
            self.annotate(test, target, to_target, to_other);
        }
    }

    // ---- switch -------------------------------------------------------------------------

    fn collect_switches(&mut self) {
        let mut found = Vec::new();
        for node in self.sem.nodes().iter() {
            let AstKind::SwitchStatement(s) = node.kind() else {
                continue;
            };
            let head = self.block_of(node.id());
            let test_blocks: Vec<Block> = s
                .cases
                .iter()
                .map(|c| self.sem.nodes().cfg_id(self.case_node(c)))
                .collect();
            let tests: Vec<Option<NodeId>> = s
                .cases
                .iter()
                .map(|c| c.test.as_ref().map(|t| t.node_id()))
                .collect();
            let mut all: BTreeSet<Block> = test_blocks.iter().copied().collect();
            all.insert(head);
            // The block the discriminant ends in is also part of the dispatch chain, but is
            // not known until the body's blocks are; a body block is never one of the
            // case-test blocks, so excluding only these is enough here.
            let bodies: Vec<Option<Block>> = test_blocks
                .iter()
                .map(|tb| {
                    let next: Vec<Block> = self
                        .succs(*tb)
                        .into_iter()
                        .filter(|(b, k)| *k == EdgeKind::Normal && !all.contains(b))
                        .map(|(b, _)| b)
                        .collect();
                    (next.len() == 1).then(|| next[0])
                })
                .collect();
            found.push(Switch {
                discriminant: s.discriminant.node_id(),
                discriminant_span: s.discriminant.span(),
                head,
                test_blocks,
                tests,
                bodies,
            });
        }
        self.switches = found;
    }

    fn case_node(&self, case: &oxc_ast::ast::SwitchCase<'_>) -> NodeId {
        case.node_id()
    }

    // ---- bodies -------------------------------------------------------------------------

    fn bodies(&self) -> Vec<(NodeId, BodyKind, Option<String>)> {
        let mut out = Vec::new();
        for node in self.sem.nodes().iter() {
            match node.kind() {
                AstKind::Program(_) => out.push((node.id(), BodyKind::Program, None)),
                AstKind::Function(f) if f.body.is_some() => out.push((
                    node.id(),
                    BodyKind::Function,
                    f.id.as_ref().map(|i| i.name.to_string()),
                )),
                AstKind::ArrowFunctionExpression(_) => {
                    out.push((node.id(), BodyKind::Arrow, None));
                }
                _ => {}
            }
        }
        out
    }

    fn rel(&self, root: NodeId, node: NodeId) -> ExprRef {
        ExprRef((node.index() - root.index()) as u32)
    }

    fn build_body(&self, id: BodyId, root: NodeId) -> BodyFlow {
        let entry_block = self.block_of(root);
        // The blocks of this body: everything reachable from its entry without entering
        // another function.
        let mut within: BTreeSet<Block> = BTreeSet::new();
        let mut stack = vec![entry_block];
        while let Some(block) = stack.pop() {
            if !self.live(block) || !within.insert(block) {
                continue;
            }
            for (next, _) in self.succs(block) {
                stack.push(next);
            }
        }
        let mut blocks: Vec<Block> = within.iter().copied().collect();
        blocks.sort_by_key(|b| b.index());
        let back = self.back_edges(entry_block, &within);

        let events = self.collect_events(root, &within);

        // Phase one: an entry node per block, then its events in order.
        let mut nodes: Vec<FlowNode> = Vec::new();
        let mut entry_of: HashMap<Block, FlowId> = HashMap::new();
        let mut exit_of: HashMap<Block, FlowId> = HashMap::new();
        let mut event_nodes: HashMap<Block, Vec<(u32, FlowId)>> = HashMap::new();
        for block in &blocks {
            let has_back = back.iter().any(|(_, target)| target == block);
            let kind = if *block == entry_block {
                FlowKind::Start
            } else if has_back {
                FlowKind::LoopHead
            } else {
                FlowKind::Join
            };
            let entry = nodes.len() as FlowId;
            nodes.push(FlowNode {
                kind,
                antecedents: Vec::new(),
            });
            entry_of.insert(*block, entry);
            let mut last = entry;
            if let Some(list) = events.get(block) {
                for (pos, assignment) in list {
                    let flow = nodes.len() as FlowId;
                    nodes.push(FlowNode {
                        kind: FlowKind::Assign(assignment.clone()),
                        antecedents: vec![Antecedent {
                            from: last,
                            edge: EdgeKind::Normal,
                        }],
                    });
                    event_nodes.entry(*block).or_default().push((*pos, flow));
                    last = flow;
                }
            }
            exit_of.insert(*block, last);
        }

        // Where each switch's discriminant has been evaluated: its own block, or, when the
        // discriminant branches (`switch (a ?? b)`), the block where those paths meet.
        let heads: Vec<[Block; 2]> = self
            .switches
            .iter()
            .map(|s| [s.head, self.end_block(s.discriminant_span, s.head, &within)])
            .collect();

        // Phase two: each entry node's antecedents, with the conditions on its edges.
        let switch_of_test: HashMap<Block, usize> = self
            .switches
            .iter()
            .enumerate()
            .flat_map(|(i, s)| s.test_blocks.iter().map(move |b| (*b, i)))
            .collect();
        for block in &blocks {
            let mut incoming: Vec<(FlowId, EdgeKind)> = Vec::new();
            let mut preds = self.preds(*block);
            preds.retain(|(p, _)| within.contains(p));

            // A switch's test blocks hear only from the dispatch chain, never from the end
            // of a case body that falls through them.
            if let Some(&index) = switch_of_test.get(block) {
                let switch = &self.switches[index];
                preds.retain(|(p, _)| heads[index].contains(p) || switch.test_blocks.contains(p));
            }
            // A case body is entered by dispatch, through a clause node, and by falling
            // through from the body above.
            let mut clause: Option<(usize, usize)> = None;
            for (si, switch) in self.switches.iter().enumerate() {
                for (ci, body) in switch.bodies.iter().enumerate() {
                    if *body == Some(*block) {
                        clause = Some((si, ci));
                    }
                }
            }
            if let Some((si, ci)) = clause {
                let switch = &self.switches[si];
                let test_block = switch.test_blocks[ci];
                let mut dispatch: Vec<Antecedent> = Vec::new();
                if within.contains(&test_block) {
                    dispatch.push(Antecedent {
                        from: exit_of[&test_block],
                        edge: EdgeKind::Normal,
                    });
                }
                let all_tests: Vec<ExprRef> = switch
                    .tests
                    .iter()
                    .flatten()
                    .map(|t| self.rel(root, *t))
                    .collect();
                let (matched, includes_default) = match switch.tests[ci] {
                    Some(t) => (vec![self.rel(root, t)], false),
                    None => (Vec::new(), true),
                };
                let clause_flow = nodes.len() as FlowId;
                nodes.push(FlowNode {
                    kind: FlowKind::Clause {
                        discriminant: self.rel(root, switch.discriminant),
                        matched,
                        includes_default,
                        all_tests,
                    },
                    antecedents: dispatch,
                });
                incoming.push((clause_flow, EdgeKind::Normal));
                // Falling through: whatever flowed into this case's test block from a body.
                for (p, _) in self.preds(test_block) {
                    let is_dispatch = heads[si].contains(&p) || switch.test_blocks.contains(&p);
                    if !is_dispatch && within.contains(&p) {
                        incoming.push((exit_of[&p], EdgeKind::Normal));
                    }
                }
                // Anything else that reaches the body (a nested construct's join) is kept.
                preds.retain(|(p, _)| *p != test_block);
            }

            for (pred, kind) in preds {
                // oxc marks the closing edge of a loop `Backedge` for `while` and `for`, but
                // a `continue` is an ordinary jump; an edge to a block still being explored
                // is a back edge whatever the builder called it.
                let kind = if back.contains(&(pred, *block)) {
                    EdgeKind::Back
                } else if kind == EdgeKind::Back {
                    EdgeKind::Normal
                } else {
                    kind
                };
                let mut from = exit_of[&pred];
                if let Some(conds) = self.conditions.get(&(pred, *block)) {
                    for cond in conds {
                        let flow = nodes.len() as FlowId;
                        nodes.push(FlowNode {
                            kind: FlowKind::Condition {
                                test: self.rel(root, cond.test),
                                assume: cond.assume,
                            },
                            antecedents: vec![Antecedent {
                                from,
                                edge: EdgeKind::Normal,
                            }],
                        });
                        from = flow;
                    }
                }
                incoming.push((from, kind));
            }
            let entry = entry_of[block] as usize;
            nodes[entry].antecedents = incoming
                .into_iter()
                .map(|(from, edge)| Antecedent { from, edge })
                .collect();
        }

        let implicit_return = blocks
            .iter()
            .find(|b| {
                self.cfg
                    .basic_block(**b)
                    .instructions()
                    .iter()
                    .any(|i| matches!(i.kind, InstructionKind::ImplicitReturn))
            })
            .filter(|_| id.kind != BodyKind::Program)
            .map(|b| exit_of[b]);

        let references = self.collect_references(root, &within, &entry_of, &event_nodes);
        BodyFlow {
            id,
            nodes,
            entry: entry_of[&entry_block],
            implicit_return,
            references,
        }
    }

    /// The edges that return to a block still being explored, found by a depth-first walk
    /// from the entry in block order.
    fn back_edges(&self, entry: Block, within: &BTreeSet<Block>) -> BTreeSet<(Block, Block)> {
        let mut back = BTreeSet::new();
        let mut state: HashMap<Block, u8> = HashMap::new();
        let mut stack: Vec<(Block, Vec<Block>, usize)> = Vec::new();
        let successors = |b: Block| -> Vec<Block> {
            self.succs(b)
                .into_iter()
                .map(|(n, _)| n)
                .filter(|n| within.contains(n))
                .collect()
        };
        state.insert(entry, 1);
        stack.push((entry, successors(entry), 0));
        while let Some((block, next, index)) = stack.pop() {
            if index < next.len() {
                let target = next[index];
                stack.push((block, next, index + 1));
                match state.get(&target) {
                    Some(1) => {
                        back.insert((block, target));
                    }
                    Some(_) => {}
                    None => {
                        state.insert(target, 1);
                        stack.push((target, successors(target), 0));
                    }
                }
            } else {
                state.insert(block, 2);
            }
        }
        back
    }

    // ---- events -------------------------------------------------------------------------

    /// The assignments of one body, by block, in the order they happen. An assignment
    /// happens when it has finished evaluating, so its position is the end of the
    /// assigning expression, and its block is the one that expression ends in.
    fn collect_events(
        &self,
        root: NodeId,
        within: &BTreeSet<Block>,
    ) -> BTreeMap<Block, Vec<(u32, Assignment)>> {
        let mut events: BTreeMap<Block, Vec<(u32, Assignment)>> = BTreeMap::new();
        let scoping = self.sem.scoping();
        let mut push = |block: Block, pos: u32, assignment: Assignment| {
            events.entry(block).or_default().push((pos, assignment));
        };

        for node in self.sem.nodes().iter() {
            let start_block = self.block_of(node.id());
            if !within.contains(&start_block) {
                continue;
            }
            match node.kind() {
                AstKind::VariableDeclarator(decl) => {
                    let Some(init) = &decl.init else { continue };
                    let block = self.end_block(init.span(), start_block, within);
                    for ident in decl.id.get_binding_identifiers() {
                        let Some(symbol) = ident.symbol_id.get() else {
                            continue;
                        };
                        push(
                            block,
                            decl.span.end,
                            Assignment {
                                target: AssignTarget::Symbol {
                                    symbol: symbol.index() as u32,
                                    name: ident.name.to_string(),
                                },
                                at: self.rel(root, node.id()),
                            },
                        );
                    }
                }
                AstKind::AssignmentExpression(assign) => {
                    // `a.b.c = v`: the root name is only read, so no write reference says so.
                    if let AssignmentTarget::StaticMemberExpression(member) = &assign.left {
                        let mut path = vec![member.property.name.to_string()];
                        let mut object = &member.object;
                        let root_ident = loop {
                            match object {
                                Expression::StaticMemberExpression(inner) => {
                                    path.push(inner.property.name.to_string());
                                    object = &inner.object;
                                }
                                Expression::Identifier(id) => break Some(id),
                                _ => break None,
                            }
                        };
                        if let Some(ident) = root_ident
                            && let Some(symbol) =
                                scoping.get_reference(ident.reference_id()).symbol_id()
                        {
                            path.reverse();
                            let block = self.end_block(assign.span, start_block, within);
                            push(
                                block,
                                assign.span.end,
                                Assignment {
                                    target: AssignTarget::Member {
                                        symbol: symbol.index() as u32,
                                        name: ident.name.to_string(),
                                        path,
                                    },
                                    at: self.rel(root, node.id()),
                                },
                            );
                        }
                    }
                }
                _ => {}
            }
        }

        // Writes to a name: `x = ...`, `x += ...`, `x++`, `for (x of ...)`, and the names in
        // a destructuring assignment.
        for index in 0..scoping.references_len() {
            let reference_id = oxc_semantic::ReferenceId::new(index);
            let reference = scoping.get_reference(reference_id);
            if !reference.is_write() {
                continue;
            }
            let Some(symbol) = reference.symbol_id() else {
                continue;
            };
            let ref_node = reference.node_id();
            let block = self.block_of(ref_node);
            if !within.contains(&block) {
                continue;
            }
            let Some((owner, span)) = self.enclosing_write(ref_node) else {
                continue;
            };
            let end_block = self.end_block(span, block, within);
            push(
                end_block,
                span.end,
                Assignment {
                    target: AssignTarget::Symbol {
                        symbol: symbol.index() as u32,
                        name: scoping.symbol_name(symbol).to_string(),
                    },
                    at: self.rel(root, owner),
                },
            );
        }

        for list in events.values_mut() {
            list.sort_by_key(|entry| entry.0);
        }
        events
    }

    /// The assignment, update or loop head a write reference belongs to, and the span whose
    /// end is when the write happens.
    fn enclosing_write(&self, node: NodeId) -> Option<(NodeId, Span)> {
        let mut current = node;
        for _ in 0..64 {
            let parent = self.sem.nodes().parent_id(current);
            if parent == current {
                return None;
            }
            match self.sem.nodes().kind(parent) {
                AstKind::AssignmentExpression(a) => return Some((parent, a.span)),
                AstKind::UpdateExpression(u) => return Some((parent, u.span)),
                AstKind::ForInStatement(f) => return Some((parent, f.left.span())),
                AstKind::ForOfStatement(f) => return Some((parent, f.left.span())),
                AstKind::Function(_)
                | AstKind::ArrowFunctionExpression(_)
                | AstKind::Program(_)
                | AstKind::ExpressionStatement(_) => return None,
                _ => {}
            }
            current = parent;
        }
        None
    }

    // ---- references ---------------------------------------------------------------------

    fn collect_references(
        &self,
        root: NodeId,
        within: &BTreeSet<Block>,
        entry_of: &HashMap<Block, FlowId>,
        event_nodes: &HashMap<Block, Vec<(u32, FlowId)>>,
    ) -> Vec<ReferenceFlow> {
        let scoping = self.sem.scoping();
        let mut out = Vec::new();
        for index in 0..scoping.references_len() {
            let reference = scoping.get_reference(oxc_semantic::ReferenceId::new(index));
            if !reference.is_read() {
                continue;
            }
            let Some(symbol) = reference.symbol_id() else {
                continue;
            };
            let node = reference.node_id();
            let block = self.block_of(node);
            if !within.contains(&block) {
                continue;
            }
            let at = self.sem.nodes().kind(node).span().start;
            let flow = event_nodes
                .get(&block)
                .and_then(|list| list.iter().rev().find(|(pos, _)| *pos <= at))
                .map(|(_, flow)| *flow)
                .unwrap_or(entry_of[&block]);
            out.push(ReferenceFlow {
                node: self.rel(root, node),
                symbol: symbol.index() as u32,
                flow,
            });
        }
        out.sort_by_key(|r| (r.node, r.flow));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// A built file with what is needed to read an `ExprRef` back as source text.
    struct Fixture {
        source: String,
        flow: FileFlow,
        spans: HashMap<(usize, u32), (u32, u32)>,
    }

    fn build(source: &str) -> Fixture {
        let allocator = oxc_allocator::Allocator::default();
        let program = crate::bridge::parse(&allocator, source, "t.ts").expect("parses");
        let semantic = crate::bridge::analyze(&program);
        let (flow, roots) = build_with_roots(&semantic, &program);
        let mut spans = HashMap::new();
        for (body, root) in roots.iter().enumerate() {
            for node in semantic.nodes().iter() {
                let id = node.id().index();
                if id >= root.index() {
                    let sp = node.kind().span();
                    spans.insert((body, (id - root.index()) as u32), (sp.start, sp.end));
                }
            }
        }
        Fixture {
            source: source.to_string(),
            flow,
            spans,
        }
    }

    impl Fixture {
        fn text(&self, body: usize, r: ExprRef) -> String {
            let (s, e) = self.spans[&(body, r.0)];
            self.source[s as usize..e as usize].to_string()
        }

        fn body(&self, name: &str) -> usize {
            self.flow
                .bodies
                .iter()
                .position(|b| b.id.name.as_deref() == Some(name))
                .unwrap_or_else(|| panic!("no body {name}"))
        }

        /// The reads of `name`, as the flow nodes they sit at, in source order.
        fn reads(&self, body: usize, name: &str) -> Vec<FlowId> {
            let b = &self.flow.bodies[body];
            b.references
                .iter()
                .filter(|r| self.text(body, r.node) == name)
                .map(|r| r.flow)
                .collect()
        }

        /// Every condition on any path from `flow` back to the start, as (test text, assume).
        fn conditions(&self, body: usize, flow: FlowId) -> Vec<(String, Assume)> {
            let b = &self.flow.bodies[body];
            let mut seen = HashSet::new();
            let mut stack = vec![flow];
            let mut out = Vec::new();
            while let Some(f) = stack.pop() {
                if !seen.insert(f) {
                    continue;
                }
                let node = &b.nodes[f as usize];
                if let FlowKind::Condition { test, assume } = &node.kind {
                    let item = (self.text(body, *test), *assume);
                    if !out.contains(&item) {
                        out.push(item);
                    }
                }
                for a in &node.antecedents {
                    stack.push(a.from);
                }
            }
            out.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
            out
        }
    }

    fn fixture(name: &str) -> Fixture {
        let path = format!("{}/tests/flow_fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        build(&std::fs::read_to_string(path).expect("fixture exists"))
    }

    /// The `case` clauses a flow node's history passes through, as `1,2` or `default`.
    fn clauses(f: &Fixture, body: usize, flow: FlowId) -> Vec<String> {
        let b = &f.flow.bodies[body];
        let mut seen = HashSet::new();
        let mut stack = vec![flow];
        let mut out = Vec::new();
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            let node = &b.nodes[id as usize];
            if let FlowKind::Clause {
                matched,
                includes_default,
                ..
            } = &node.kind
            {
                let mut label: Vec<String> = matched.iter().map(|m| f.text(body, *m)).collect();
                if *includes_default {
                    label.push("default".to_string());
                }
                out.push(label.join(","));
            }
            for a in &node.antecedents {
                stack.push(a.from);
            }
        }
        out.sort();
        out
    }

    fn read_flows(f: &Fixture, body: usize, name: &str) -> Vec<FlowId> {
        f.reads(body, name)
    }

    #[test]
    fn short_circuit_operand_is_entered_only_when_the_left_holds() {
        let f = fixture("branches.ts");
        let b = f.body("branches");
        let y = read_flows(&f, b, "y");
        // `y > 0`, the else-branch `y`, and the ternary's `y`.
        assert_eq!(
            f.conditions(b, y[0]),
            vec![("x !== null".to_string(), Assume::True)]
        );
    }

    #[test]
    fn if_branches_carry_the_whole_test() {
        let f = fixture("branches.ts");
        let b = f.body("branches");
        let whole = "x !== null && y > 0".to_string();
        let then_x = read_flows(&f, b, "x")[1];
        assert!(
            f.conditions(b, then_x)
                .contains(&(whole.clone(), Assume::True))
        );
        assert!(
            !f.conditions(b, then_x)
                .contains(&(whole.clone(), Assume::False))
        );
        let else_y = read_flows(&f, b, "y")[1];
        assert!(
            f.conditions(b, else_y)
                .contains(&(whole.clone(), Assume::False))
        );
        assert!(!f.conditions(b, else_y).contains(&(whole, Assume::True)));
    }

    #[test]
    fn ternary_arms_and_nullish_edges() {
        let f = fixture("branches.ts");
        let b = f.body("branches");
        // The ternary `x ? x.length : y`: `x` in the true arm, `y` in the false arm.
        let arm_x = *read_flows(&f, b, "x").last().unwrap();
        assert!(
            f.conditions(b, arm_x)
                .contains(&("x".to_string(), Assume::True))
        );
        let arm_y = *read_flows(&f, b, "y").last().unwrap();
        assert!(
            f.conditions(b, arm_y)
                .contains(&("x".to_string(), Assume::False))
        );
        // `x ?? "d"` leaves both a Nullish and a NotNullish edge.
        let z = read_flows(&f, b, "z")[0];
        let conds = f.conditions(b, z);
        assert!(conds.contains(&("x".to_string(), Assume::Nullish)));
        assert!(conds.contains(&("x".to_string(), Assume::NotNullish)));
        // The declarator is an assignment event of `z`, ordered before the read.
        let node = &f.flow.bodies[b].nodes[z as usize];
        assert!(matches!(
            &node.kind,
            FlowKind::Assign(Assignment { target: AssignTarget::Symbol { name, .. }, .. }) if name == "z"
        ));
    }

    #[test]
    fn loops_have_heads_back_edges_and_exit_conditions() {
        let f = fixture("loops.ts");
        let b = f.body("loops");
        let body = &f.flow.bodies[b];
        let mut back = 0;
        for node in &body.nodes {
            let has_back = node.antecedents.iter().any(|a| a.edge == EdgeKind::Back);
            if has_back {
                back += 1;
            }
        }
        assert_eq!(back, 3, "while, for and do-while each close one loop");
        for node in &body.nodes {
            if node.antecedents.iter().any(|a| a.edge == EdgeKind::Back) {
                assert!(
                    matches!(node.kind, FlowKind::LoopHead),
                    "a node entered by a back edge is a loop head"
                );
            }
        }
        // After `if (n == 3) break;` inside `while (n > 0)`.
        let s_in_while = read_flows(&f, b, "s")[0];
        assert_eq!(
            f.conditions(b, s_in_while),
            vec![
                ("n == 3".to_string(), Assume::False),
                ("n > 0".to_string(), Assume::True)
            ]
        );
        // The `i` read in the for body is inside `i < n`; `return n` is after `while (s)`.
        let i_in_body = *read_flows(&f, b, "i").last().unwrap();
        assert!(
            f.conditions(b, i_in_body)
                .contains(&("i < n".to_string(), Assume::True))
        );
        let after = *read_flows(&f, b, "n").last().unwrap();
        assert!(
            f.conditions(b, after)
                .contains(&("s".to_string(), Assume::False))
        );
    }

    #[test]
    fn switch_cases_are_entered_by_dispatch_and_by_falling_through() {
        let f = fixture("switches.ts");
        let b = f.body("sw");
        let ks = read_flows(&f, b, "k");
        // The discriminant `k` of the switch itself is not a read inside a case; the three
        // reads are the bodies: `case 1: case 2:`, `default:`, `case 3:`.
        let bodies: Vec<Vec<String>> = ks
            .iter()
            .filter(|flow| !clauses(&f, b, **flow).is_empty())
            .map(|flow| clauses(&f, b, *flow))
            .collect();
        assert_eq!(bodies.len(), 3);
        // An empty case falls into the next: both tests reach the shared body.
        assert_eq!(bodies[0], vec!["1".to_string(), "2".to_string()]);
        // `default` is entered by dispatch only.
        assert_eq!(bodies[1], vec!["default".to_string()]);
        // `case 3` is entered by dispatch and by falling out of `default`, not out of `1`/`2`
        // (that body ends in `break`).
        assert_eq!(bodies[2], vec!["3".to_string(), "default".to_string()]);
        // `default` knows what none of the tests were.
        let default_clause = f.flow.bodies[b]
            .nodes
            .iter()
            .find_map(|n| match &n.kind {
                FlowKind::Clause {
                    includes_default: true,
                    all_tests,
                    ..
                } => Some(all_tests.clone()),
                _ => None,
            })
            .expect("a default clause");
        let texts: Vec<String> = default_clause.iter().map(|t| f.text(b, *t)).collect();
        assert_eq!(texts, vec!["1", "2", "3"]);
    }

    #[test]
    fn a_switch_whose_discriminant_branches_still_dispatches() {
        let f = fixture("switches.ts");
        let b = f.body("sw2");
        let a = read_flows(&f, b, "a");
        // `a` in `case 1:` is entered through the clause for `1`, and `b` in `default`
        // through the default clause.
        let case_one = *a.last().unwrap();
        assert_eq!(clauses(&f, b, case_one), vec!["1".to_string()]);
        let in_default = *read_flows(&f, b, "b").last().unwrap();
        assert_eq!(clauses(&f, b, in_default), vec!["default".to_string()]);
    }

    #[test]
    fn assignments_are_ordered_against_reads() {
        let f = fixture("assignments.ts");
        let b = f.body("assign");
        let body = &f.flow.bodies[b];
        let v = read_flows(&f, b, "v");
        assert_eq!(v.len(), 4);
        // `v;` right after `let v = a`, and the `v` inside `v + "x"`, see the declaration.
        assert_eq!(v[0], v[1]);
        // After `v = v + "x"` it is the new assignment, a different node.
        assert_ne!(v[2], v[0]);
        assert!(matches!(
            body.nodes[v[2] as usize].kind,
            FlowKind::Assign(_)
        ));
        // After the `if`, paths meet: the assignment inside it and the path that skipped it.
        let joined = &body.nodes[v[3] as usize];
        assert!(matches!(joined.kind, FlowKind::Join));
        let kinds: Vec<bool> = joined
            .antecedents
            .iter()
            .map(|a| matches!(body.nodes[a.from as usize].kind, FlowKind::Assign(_)))
            .collect();
        assert!(kinds.contains(&true) && kinds.contains(&false));
        // `o.p.q = "s"` is an assignment to a member path; the read after it sees it.
        let o = *read_flows(&f, b, "o").last().unwrap();
        assert!(matches!(
            &body.nodes[o as usize].kind,
            FlowKind::Assign(Assignment { target: AssignTarget::Member { name, path, .. }, .. })
                if name == "o" && path == &vec!["p".to_string(), "q".to_string()]
        ));
    }

    #[test]
    fn try_catch_finally_edges_are_marked() {
        let f = fixture("exceptions.ts");
        let b = f.body("tcf");
        let body = &f.flow.bodies[b];
        let exception = body
            .nodes
            .iter()
            .filter(|n| n.antecedents.iter().any(|a| a.edge == EdgeKind::Exception))
            .count();
        assert_eq!(
            exception, 1,
            "one catch block is entered from the try block"
        );
        let finalize = body
            .nodes
            .iter()
            .find(|n| n.antecedents.iter().any(|a| a.edge == EdgeKind::Finalize))
            .expect("a finally block");
        assert_eq!(
            finalize.antecedents.len(),
            2,
            "from the try block and from the catch block"
        );
    }

    #[test]
    fn nested_functions_are_bodies_of_their_own_and_dead_code_is_absent() {
        let f = fixture("bodies.ts");
        assert_eq!(
            f.flow.bodies.len(),
            5,
            "program, outer, the arrow, g, falls"
        );
        let outer = f.body("outer");
        let g = f.body("g");
        let falls = f.body("falls");
        // `x` is read once, inside `g`; the `x;` after `return` is dead and not in `outer`.
        assert!(f.reads(outer, "x").is_empty());
        assert_eq!(f.reads(g, "x").len(), 1);
        // `return f(1)` ends `outer`; `g` returns; `falls` and the arrow can fall off the end.
        assert!(f.flow.bodies[outer].implicit_return.is_none());
        assert!(f.flow.bodies[g].implicit_return.is_none());
        assert!(f.flow.bodies[falls].implicit_return.is_some());
        let arrow = f
            .flow
            .bodies
            .iter()
            .find(|b| b.id.kind == BodyKind::Arrow)
            .unwrap();
        assert!(arrow.implicit_return.is_some());
    }

    #[test]
    fn the_same_source_builds_the_same_graph() {
        for name in FIXTURES {
            let path = format!("{}/tests/flow_fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
            let source = std::fs::read_to_string(path).unwrap();
            assert_eq!(build(&source).flow, build(&source).flow, "{name}");
        }
    }

    #[test]
    fn a_comment_above_does_not_change_the_graph() {
        for name in FIXTURES {
            let path = format!("{}/tests/flow_fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
            let source = std::fs::read_to_string(path).unwrap();
            let shifted = format!("// a new first line\n\n{source}");
            assert_eq!(build(&source).flow, build(&shifted).flow, "{name}");
        }
    }

    const FIXTURES: [&str; 6] = [
        "branches.ts",
        "loops.ts",
        "switches.ts",
        "assignments.ts",
        "exceptions.ts",
        "bodies.ts",
    ];

    /// Properties every graph must have, whatever the source.
    fn check_invariants(flow: &FileFlow, label: &str) {
        for body in &flow.bodies {
            let n = body.nodes.len() as u32;
            assert_eq!(
                body.nodes[body.entry as usize].kind,
                FlowKind::Start,
                "{label}"
            );
            for (index, node) in body.nodes.iter().enumerate() {
                for a in &node.antecedents {
                    assert!(a.from < n, "{label}: dangling antecedent f{}", a.from);
                }
                if node.antecedents.is_empty() {
                    assert!(
                        matches!(node.kind, FlowKind::Start),
                        "{label}: f{index} {:?} has no antecedent",
                        node.kind
                    );
                }
            }
            for r in &body.references {
                assert!(r.flow < n, "{label}: reference at a missing node");
            }
            if let Some(end) = body.implicit_return {
                assert!(end < n, "{label}");
            }
            // Without its back edges a flow graph has no cycle.
            let mut state = vec![0u8; body.nodes.len()];
            for start in 0..body.nodes.len() {
                let mut stack = vec![(start, 0usize)];
                while let Some((node, next)) = stack.pop() {
                    if next == 0 {
                        if state[node] == 2 {
                            continue;
                        }
                        assert_ne!(state[node], 1, "{label}: a cycle without a back edge");
                        state[node] = 1;
                    }
                    let ants: Vec<usize> = body.nodes[node]
                        .antecedents
                        .iter()
                        .filter(|a| a.edge != EdgeKind::Back)
                        .map(|a| a.from as usize)
                        .collect();
                    if next < ants.len() {
                        stack.push((node, next + 1));
                        stack.push((ants[next], 0));
                    } else {
                        state[node] = 2;
                    }
                }
            }
        }
    }

    fn all_ts_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                all_ts_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "ts" || e == "tsx") {
                out.push(path);
            }
        }
    }

    fn invariants_over(dir: &std::path::Path) -> usize {
        let mut files = Vec::new();
        all_ts_files(dir, &mut files);
        let mut built = 0;
        for path in files {
            let Ok(source) = std::fs::read_to_string(&path) else {
                continue;
            };
            if std::env::var_os("TSOX_FLOW_TRACE").is_some() {
                eprintln!("{}", path.display());
            }
            let Ok(flow) = build_file_flow(&source, &path.to_string_lossy()) else {
                continue;
            };
            check_invariants(&flow, &path.to_string_lossy());
            built += 1;
        }
        built
    }

    #[test]
    fn the_flow_fixtures_satisfy_the_invariants() {
        for name in FIXTURES {
            let f = fixture(name);
            check_invariants(&f.flow, name);
        }
    }

    #[test]
    fn every_repository_fixture_builds_a_valid_graph() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
        let built = invariants_over(&root);
        assert!(
            built > 100,
            "only {built} fixtures were found under {root:?}"
        );
    }

    /// Prints the graph of one file: `TSOX_FLOW_FILE=x.ts cargo test ... flow::tests::show`.
    #[test]
    #[ignore]
    fn show() {
        let path = std::env::var("TSOX_FLOW_FILE").expect("set TSOX_FLOW_FILE");
        let f = build(&std::fs::read_to_string(path).unwrap());
        println!("{}", f.flow.dump());
    }

    /// Run by hand against a larger corpus: `TSOX_FLOW_CORPUS=/path cargo test -p
    /// tsox-checker --lib flow::tests::corpus -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn corpus() {
        let dir = std::env::var("TSOX_FLOW_CORPUS").expect("set TSOX_FLOW_CORPUS");
        let built = invariants_over(std::path::Path::new(&dir));
        println!("flow graphs built and checked: {built}");
    }
}
