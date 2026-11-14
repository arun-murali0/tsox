// Which statements can never run, and which variables are read before they are assigned,
// decided the way tsc's binder decides it.
//
// tsc marks a statement unreachable while it binds: every `return`, `throw`, `break` and
// `continue` leaves the "current flow" unreachable, and so does a loop or `if` whose exits
// are all closed off. The checker then reports one TS7027 for each run of adjacent dead
// statements. This module replays exactly that, over the oxc AST, and nothing else. It is
// arena-free and a pure function of the program, so the answer can be shared by anything that
// asks "is this statement dead" or "is this variable assigned here": the TS7027 report, the
// TS2454 report and, later, the narrowing resolver.
//
// Why not take it from oxc's control-flow graph? oxc builds a correct graph of JavaScript, and
// tsc's binder is not the same graph. The differences are small and every one of them changes
// which statements tsc calls dead, or which assignments reach a read:
//
//   * a condition is constant only when it is the bare `true` or `false` keyword; `(true)`,
//     `!true`, `1` and `null` are not, but `&&` and `||` pass constants through their operands
//     and `??` does not;
//   * `break` and `continue` jump straight to their target, past any enclosing `finally`;
//   * a `catch` and a `finally` are entered from the start of their `try`, so they are live
//     whenever the `try` is, whatever its body does, and what the `try` assigned is not
//     assigned in them;
//   * a `switch` without `default` always falls out of the bottom.
//
// oxc's graph knows none of these constants. So the flow here is computed on the AST, and oxc's
// graph stays what it is good for (blocks and the node-to-block map) in the later phases. See
// docs/architecture/flow.md.
//
// The state carried through the walk is "is this point reachable" and "which tracked variables
// are definitely assigned". Two states meet by keeping the variables both have; a dead state
// meets another by giving way to it. Assignments only ever add variables, so a loop needs no
// fixpoint: what is assigned at the top of a loop is what was assigned on the way in.
//
// A tracked variable is a `let` or `var` with a type annotation and no initializer. Whether a
// read of one is an error also depends on its declared type (`number | undefined` is fine),
// which only the checker knows, so this module records the reads and the checker decides.
//
// Not covered, because they need types: a call to a function that returns `never`, an
// `asserts false` call, a `switch` that exhausts its discriminant, and a condition that narrows
// a variable by anything but the forms listed in `removes_undefined`.

use std::collections::{HashMap, HashSet};

use oxc_ast::ast::{
    AssignmentExpression, AssignmentOperator, AssignmentTarget, BindingPattern,
    ConditionalExpression, Declaration, ExportDefaultDeclarationKind, Expression, ForStatementInit,
    ForStatementLeft, IdentifierReference, LabeledStatement, LogicalExpression, LogicalOperator,
    Program, SimpleAssignmentTarget, Statement, SwitchStatement, TSNamespaceDeclaration,
    TSNamespaceDeclarationBody, TryStatement, UnaryOperator, UpdateExpression,
    VariableDeclarationKind,
};
use oxc_ast_visit::{Visit, walk};
use oxc_semantic::Scoping;
use oxc_semantic::SymbolId;
use oxc_span::{GetSpan, Span};

/// The result of one pass over a file.
#[derive(Debug, Default)]
pub struct Reachability {
    /// Start offsets of every statement bound while the flow was unreachable. A function body
    /// inside dead code is its own container and starts reachable, so its statements are not
    /// here unless they are dead for their own reasons.
    pub dead: HashSet<u32>,
    /// One span per TS7027: from the first statement of a run of dead statements to the end of
    /// the last. In source order within each statement list.
    pub reports: Vec<Span>,
    /// One span per `case` or `default` clause that has statements, is not the last clause, and
    /// can run off its end into the next one (TS7029, under `noFallthroughCasesInSwitch`). A
    /// clause with no statements is just a stacked label and is never one.
    pub fallthroughs: Vec<Span>,
    /// The identifiers that read a tracked variable on a path where it may not have been
    /// assigned, in the order they are met. Whether each is TS2454 is up to the checker, which
    /// knows the declared type: `let x: number | undefined;` may be read freely.
    pub unassigned: Vec<UnassignedRead>,
    /// What the flow says about each function that has a block body, in the order they are met:
    /// whether the end of the body can be reached and whether a `return` can. The checker pairs
    /// these with the return types it knows.
    pub functions: Vec<FunctionFact>,
}

/// The flow facts about one function with a block body.
#[derive(Debug, Clone)]
pub struct FunctionFact {
    /// Where the function node starts: the key the checker records its return types under.
    pub key: u32,
    /// Where an error about the whole function is reported: its return type annotation when it
    /// has one, otherwise its name or, failing that, its start.
    pub error_span: Span,
    /// Can the end of the body be reached? tsc's `HasImplicitReturn`.
    pub end_reachable: bool,
    /// Is there a `return` statement that can itself be reached? tsc's `HasExplicitReturn`: a
    /// bare `return;` counts, one in dead code does not.
    pub has_explicit_return: bool,
    /// Every `return;` with no value, reachable or not: tsc checks each against the return type.
    pub bare_returns: Vec<Span>,
    pub has_annotation: bool,
    /// Does any `return` carry a value other than `undefined` or `void ...`? Read from the syntax,
    /// so it cannot see that a value has type `any`. Dead returns count, as they do for tsc's
    /// inferred type.
    pub returns_a_value: bool,
    /// For an async function annotated `Promise<T>`, what the syntax of `T` says on its own.
    pub async_inner: Option<PromiseInner>,
    pub is_async: bool,
    pub is_generator: bool,
    pub is_constructor: bool,
}

/// What the `T` of an annotation `Promise<T>` is, as far as its syntax says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromiseInner {
    /// `void`, `any`, `undefined`, or a union with `void` or `any`: nothing needs returning.
    VoidUndefinedOrAny,
    Never,
    /// `unknown`, or a union with `undefined`: `undefined` is a value of it.
    AcceptsUndefined,
    /// A keyword type, a literal, a shape: `undefined` is not a value of it.
    RejectsUndefined,
}

// Reads the `T` of `Promise<T>` from an annotation, when it can be read from syntax alone: a name
// that has to be resolved (an alias, an interface) says nothing.
fn promise_inner(annotation: &oxc_ast::ast::TSType<'_>) -> Option<PromiseInner> {
    use oxc_ast::ast::{TSType, TSTypeName};
    let TSType::TSTypeReference(reference) = annotation else {
        return None;
    };
    let TSTypeName::IdentifierReference(name) = &reference.type_name else {
        return None;
    };
    if name.name != "Promise" {
        return None;
    }
    let arguments = reference.type_arguments.as_ref()?;
    let [argument] = arguments.params.as_slice() else {
        return None;
    };
    classify_inner(argument)
}

fn classify_inner(ty: &oxc_ast::ast::TSType<'_>) -> Option<PromiseInner> {
    use oxc_ast::ast::TSType;
    Some(match ty {
        TSType::TSVoidKeyword(_) | TSType::TSAnyKeyword(_) | TSType::TSUndefinedKeyword(_) => {
            PromiseInner::VoidUndefinedOrAny
        }
        TSType::TSNeverKeyword(_) => PromiseInner::Never,
        TSType::TSUnknownKeyword(_) => PromiseInner::AcceptsUndefined,
        TSType::TSNumberKeyword(_)
        | TSType::TSStringKeyword(_)
        | TSType::TSBooleanKeyword(_)
        | TSType::TSBigIntKeyword(_)
        | TSType::TSSymbolKeyword(_)
        | TSType::TSObjectKeyword(_)
        | TSType::TSNullKeyword(_)
        | TSType::TSLiteralType(_)
        | TSType::TSTypeLiteral(_)
        | TSType::TSArrayType(_)
        | TSType::TSTupleType(_)
        | TSType::TSFunctionType(_) => PromiseInner::RejectsUndefined,
        TSType::TSUnionType(union) => {
            let mut result = PromiseInner::RejectsUndefined;
            for member in &union.types {
                match classify_inner(member)? {
                    // `void` (and `any`, which swallows the rest) settles it.
                    PromiseInner::VoidUndefinedOrAny => {
                        if matches!(member, TSType::TSVoidKeyword(_) | TSType::TSAnyKeyword(_)) {
                            return Some(PromiseInner::VoidUndefinedOrAny);
                        }
                        result = PromiseInner::AcceptsUndefined;
                    }
                    PromiseInner::AcceptsUndefined => result = PromiseInner::AcceptsUndefined,
                    PromiseInner::Never | PromiseInner::RejectsUndefined => {}
                }
            }
            result
        }
        _ => return None,
    })
}

// What is known about the function being walked so far.
#[derive(Default)]
struct FnState {
    explicit_return: bool,
    bare_returns: Vec<Span>,
    returns_a_value: bool,
}

/// A read of a tracked variable that no assignment is known to reach.
#[derive(Debug, Clone)]
pub struct UnassignedRead {
    pub span: Span,
    pub symbol: SymbolId,
    pub name: String,
    /// What the declaration's annotation says on its own about reading it unassigned, for when
    /// the checker has no type for the variable (it never walked the body it is declared in).
    pub allows_unassigned: Option<bool>,
}

impl Reachability {
    pub fn is_dead(&self, statement_start: u32) -> bool {
        self.dead.contains(&statement_start)
    }
}

/// Runs the flow over a file. `scoping` resolves identifiers to variables; it is the same
/// scope analysis the checker already has.
pub fn analyze(program: &Program<'_>, scoping: &Scoping) -> Reachability {
    analyze_as(program, scoping, false)
}

// `forced_module` is `moduleDetection: "force"`: a file with no import or export is still a module,
// so a `let` at its top is local to it, not a global.
pub fn analyze_as(program: &Program<'_>, scoping: &Scoping, forced_module: bool) -> Reachability {
    // Two walks. The first only learns which variables are tracked and which function each
    // lives in, so that a read that comes before its declaration (a hoisted `var`) is known to
    // be of a tracked variable. Its other results are thrown away.
    let mut discover = Analyzer::new(scoping, HashMap::new(), HashMap::new(), HashSet::new());
    discover.is_script = program.source_type.is_script() && !forced_module;
    let predicates = predicate_functions(program);
    discover.predicates = predicates.clone();
    let mut namespaces = NamespaceMembers::new();
    collect_namespaces(&program.body, &mut namespaces);
    discover.namespaces = namespaces.clone();
    discover.visit_program(program);
    let mut analyzer = Analyzer::new(
        scoping,
        discover.tracked,
        discover.last_assignment,
        discover.definite,
    );
    analyzer.is_script = program.source_type.is_script() && !forced_module;
    analyzer.predicates = predicates;
    analyzer.namespaces = namespaces;
    analyzer.discovering = false;
    analyzer.visit_program(program);
    analyzer.out
}

// The functions of this file whose return type is a type predicate: `function f(x): x is T`
// and `const f = (x): x is T => ...`.
fn predicate_functions(program: &Program<'_>) -> HashSet<SymbolId> {
    struct Scan(HashSet<SymbolId>);
    fn is_predicate(
        annotation: &Option<oxc_allocator::Box<'_, oxc_ast::ast::TSTypeAnnotation<'_>>>,
    ) -> bool {
        annotation.as_ref().is_some_and(|annotation| {
            matches!(
                annotation.type_annotation,
                oxc_ast::ast::TSType::TSTypePredicate(_)
            )
        })
    }
    impl<'a> Visit<'a> for Scan {
        fn visit_function(
            &mut self,
            function: &oxc_ast::ast::Function<'a>,
            flags: oxc_semantic::ScopeFlags,
        ) {
            if is_predicate(&function.return_type)
                && let Some(id) = function.id.as_ref().and_then(|id| id.symbol_id.get())
            {
                self.0.insert(id);
            }
            walk::walk_function(self, function, flags);
        }
        fn visit_variable_declarator(&mut self, declarator: &oxc_ast::ast::VariableDeclarator<'a>) {
            if let (oxc_ast::ast::BindingPattern::BindingIdentifier(id), Some(init)) =
                (&declarator.id, &declarator.init)
            {
                let predicate = match init {
                    Expression::ArrowFunctionExpression(arrow) => is_predicate(&arrow.return_type),
                    Expression::FunctionExpression(function) => is_predicate(&function.return_type),
                    _ => false,
                };
                if predicate && let Some(symbol) = id.symbol_id.get() {
                    self.0.insert(symbol);
                }
            }
            walk::walk_variable_declarator(self, declarator);
        }
    }
    let mut scan = Scan(HashSet::new());
    scan.visit_program(program);
    scan.0
}

// The variables that are definitely assigned: a set of small indexes, one per tracked variable.
#[derive(Clone, Default, PartialEq, Debug)]
struct Bits(Vec<u64>);

impl Bits {
    fn has(&self, index: usize) -> bool {
        self.0
            .get(index / 64)
            .is_some_and(|word| word >> (index % 64) & 1 == 1)
    }

    fn set(&mut self, index: usize) {
        if self.0.len() <= index / 64 {
            self.0.resize(index / 64 + 1, 0);
        }
        self.0[index / 64] |= 1 << (index % 64);
    }

    fn clear(&mut self, index: usize) {
        if let Some(word) = self.0.get_mut(index / 64) {
            *word &= !(1 << (index % 64));
        }
    }

    // Keep only what `other` also has.
    fn intersect(&mut self, other: &Bits) {
        for (i, word) in self.0.iter_mut().enumerate() {
            *word &= other.0.get(i).copied().unwrap_or(0);
        }
    }

    fn union(&mut self, other: &Bits) {
        if self.0.len() < other.0.len() {
            self.0.resize(other.0.len(), 0);
        }
        for (i, word) in other.0.iter().enumerate() {
            self.0[i] |= word;
        }
    }
}

// The flow at one point: can it be reached, and what is definitely assigned there.
#[derive(Clone, Default, Debug)]
struct St {
    live: bool,
    assigned: Bits,
}

impl St {
    fn dead() -> St {
        St {
            live: false,
            assigned: Bits::default(),
        }
    }

    fn live() -> St {
        St {
            live: true,
            assigned: Bits::default(),
        }
    }

    // Where two paths meet. A dead path adds nothing; two live ones keep what both assigned.
    fn join(a: &St, b: &St) -> St {
        match (a.live, b.live) {
            (false, _) => b.clone(),
            (_, false) => a.clone(),
            _ => {
                let mut met = a.clone();
                met.assigned.intersect(&b.assigned);
                met
            }
        }
    }
}

// A variable whose reads are checked: where it is, and which function it lives in.
#[derive(Clone, Copy, Debug)]
struct Tracked {
    index: usize,
    container: usize,
    // Where its declaration starts, to find the end of the last assignment (tsc's
    // `extendAssignmentPosition` stops climbing at the declaration).
    declaration_start: u32,
    // The function its declaration is written in, counting a function that is called where it is
    // written: tsc's `markNodeAssignments` compares the nearest function around each assignment,
    // and for that an immediately invoked function is a function of its own.
    function: usize,
    // tsc's `isMutableLocalVariableDeclaration`: a `let` that is not exported and not at the top
    // of a script. Only such a variable can be "never initialized" for the closure rule.
    mutable_local: bool,
    // Whether the annotation lets the variable be read before it is assigned (`any`, `unknown`,
    // `void`, or a union with `undefined`), when its syntax says so on its own. `None` for a
    // name that has to be resolved (a type alias), which only the checker can do.
    allows_unassigned: Option<bool>,
}

// A function that is not called where it is written, entered while the walk is inside it. What
// the flow looked like where it was created matters to a read inside it of a variable that is
// not assigned anywhere: tsc continues from there when the function is an arrow or a function
// expression and the read is past the last assignment.
#[derive(Clone, Copy)]
struct Kind {
    climbable: bool,
    transparent: bool,
}

impl Kind {
    const OWN_FLOW: Kind = Kind {
        climbable: false,
        transparent: false,
    };
}

struct Frame {
    // Arrow functions and function expressions only; a function declaration, a method and a field
    // initializer start a flow of their own.
    climbable: bool,
    // Called where it is written but async or a generator, so its body is not part of the flow
    // around it (it can suspend) yet tsc still counts it as belonging to the function it is in
    // when deciding which flow a read of an outer variable follows.
    transparent: bool,
    // The function it was created in, and the flow there at that point.
    parent: usize,
    creation: St,
}

// A jump target, kept as an index into `Analyzer::arrivals`: what the flow looked like on every
// path that arrived there while reachable, met together. tsc's `addAntecedent` ignores
// unreachable antecedents, so "has an antecedent" is exactly "was reached while reachable".
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

struct Analyzer<'s> {
    scoping: &'s Scoping,
    // The flow at the point being visited. tsc's `currentFlow`.
    st: St,
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
    // The name (or start) of the class method being entered and whether it is a constructor.
    method: Option<(Span, bool)>,
    // The next property key visited belongs to a method or accessor: see `visit_property_key`.
    member_key: bool,
    fn_state: FnState,
    arrivals: Vec<Option<Bits>>,
    // How many label statements sit directly around the statement being entered, so a loop can
    // make them its `continue` targets. tsc's `setContinueTarget`.
    pending_labels: usize,
    // The function body being walked, numbered in the order they are entered. A function that is
    // called where it is written does not start a new one: tsc's flow container.
    container: usize,
    containers: usize,
    // The nearest function around the point being walked, numbered as they are entered.
    function: usize,
    functions: usize,
    // Learning which variables are tracked (first walk), or reporting (second).
    discovering: bool,
    tracked: HashMap<SymbolId, Tracked>,
    // The kind and `declare` flag of the declaration being walked, and whether it is the head of
    // a `for...in` / `for...of`, whose variable is assigned by the loop.
    declaration: Option<(VariableDeclarationKind, bool)>,
    in_for_head: bool,
    // For each enclosing `for (let x in a)`: the symbol of `x` and of `a`. See `visit_computed_member_expression`.
    for_in_keys: Vec<(SymbolId, SymbolId)>,
    // Variables whose initializer is being visited: a read there is of a type still being resolved.
    initializing: Vec<SymbolId>,
    // Variables with an ambient (`declare var`) declaration: tsc judges a variable by its first
    // declaration, so a later one in the same file does not make it a candidate.
    ambient_vars: HashSet<SymbolId>,
    in_export: bool,
    // The variables something assigns with a plain `x = ...`, a logical assignment, a destructuring
    // or a loop head: tsc's "definite" assignment, as opposed to `x += 1` and `x++`. Learned in
    // the first walk.
    definite: HashSet<SymbolId>,
    // The closures being walked, innermost last.
    frames: Vec<Frame>,
    // The statements around the point being walked that tsc extends an assignment's position
    // over, outermost first.
    statements: Vec<Span>,
    // For each tracked variable, where its last assignment ends (`u32::MAX` when one is in
    // another function). Learned in the first walk. tsc's `lastAssignmentPos`.
    last_assignment: HashMap<SymbolId, u32>,
    is_script: bool,
    // Functions declared with a type predicate (`x is T`) as their return type, which narrow
    // their argument where a call to one is a condition.
    predicates: HashSet<SymbolId>,
    namespaces: NamespaceMembers,
    out: Reachability,
}

impl<'s> Analyzer<'s> {
    fn new(
        scoping: &'s Scoping,
        tracked: HashMap<SymbolId, Tracked>,
        last_assignment: HashMap<SymbolId, u32>,
        definite: HashSet<SymbolId>,
    ) -> Self {
        Self {
            scoping,
            predicates: HashSet::new(),
            namespaces: HashMap::new(),
            st: St::live(),
            within: false,
            jumps: Jumps::default(),
            return_target: None,
            iife_pending: false,
            in_place_function: None,
            method: None,
            member_key: false,
            fn_state: FnState::default(),
            arrivals: Vec::new(),
            pending_labels: 0,
            container: 0,
            containers: 0,
            function: 0,
            functions: 0,
            discovering: true,
            tracked,
            declaration: None,
            in_for_head: false,
            for_in_keys: Vec::new(),
            initializing: Vec::new(),
            ambient_vars: HashSet::new(),
            in_export: false,
            definite,
            frames: Vec::new(),
            statements: Vec::new(),
            last_assignment,
            is_script: false,
            out: Reachability::default(),
        }
    }

    // Enters a function, returning the one that was around it to put back afterwards.
    fn enter_function(&mut self) -> usize {
        self.functions += 1;
        std::mem::replace(&mut self.function, self.functions)
    }

    fn new_target(&mut self) -> Target {
        self.arrivals.push(None);
        self.arrivals.len() - 1
    }

    // The current flow arrives at `target`, if it can be reached.
    fn arrive(&mut self, target: Target) {
        if !self.st.live {
            return;
        }
        let bits = self.st.assigned.clone();
        self.arrive_with(target, &bits);
    }

    fn arrive_with(&mut self, target: Target, bits: &Bits) {
        match &mut self.arrivals[target] {
            Some(met) => met.intersect(bits),
            slot @ None => *slot = Some(bits.clone()),
        }
    }

    // What the flow is at `target`: dead unless something arrived.
    fn at(&self, target: Target) -> St {
        match &self.arrivals[target] {
            Some(bits) => St {
                live: true,
                assigned: bits.clone(),
            },
            None => St::dead(),
        }
    }

    // A function, class static block or namespace body is a flow container: it has no jump
    // targets of the code around it and says nothing about their labels.
    //
    // A function that is not immediately invoked starts reachable and leaves the surrounding
    // flow as it was. An immediately invoked one (`(() => { ... })()`, not async, not a generator)
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
        kind: Kind,
        body: impl FnOnce(&mut Self),
    ) {
        let (climbable, transparent) = (kind.climbable, kind.transparent);
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
            self.st = self.at(exit);
        } else {
            // An async or generator function called where it is written is reachable on its
            // own (it can suspend), but for the question of what a variable holds tsc counts it
            // as part of the function around it: a read inside walks back through its start into
            // the flow where it was created. So it keeps the surrounding function's container
            // and starts with what was assigned there.
            let inherited = if transparent {
                St {
                    live: true,
                    assigned: self.st.assigned.clone(),
                }
            } else {
                St::live()
            };
            let outer = std::mem::replace(&mut self.st, inherited);
            let outer_container = self.container;
            if !transparent {
                self.containers += 1;
                self.container = self.containers;
            }
            self.return_target = None;
            self.frames.push(Frame {
                climbable,
                transparent,
                parent: outer_container,
                creation: outer.clone(),
            });
            body(self);
            self.frames.pop();
            self.container = outer_container;
            self.st = outer;
        }
        self.return_target = outer_return;
        self.within = within;
        self.jumps = jumps;
        self.pending_labels = pending;
    }

    // A statement that is not in a statement list (the arm of an `if`, a loop body, the body of
    // a label). tsc reports such a dead statement on its own, as a run of one.
    fn child<'a>(&mut self, statement: &Statement<'a>) {
        let dead = !self.st.live;
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

    // The flow on each edge out of a condition, given the flow that enters it: the state if it
    // is true and the state if it is false, either of which can be dead. tsc's
    // `createFlowCondition` makes an edge unreachable only for the bare keyword, and
    // `bindLogicalLikeExpression` threads the rest. An operand that is not itself logical is
    // evaluated (it may assign) and its two edges differ only by what the test tells about a
    // variable it names (`removes_undefined`).
    fn condition<'a>(
        &mut self,
        expression: &Expression<'a>,
        entry: St,
        parent_is_nullish: bool,
    ) -> (St, St) {
        if !entry.live {
            // Nothing here runs, but what is written here still counts: an assignment in it is an
            // assignment of the variable, and a function in it has a body to walk.
            self.st = entry;
            self.visit_expression(expression);
            return (St::dead(), St::dead());
        }
        match expression {
            Expression::BooleanLiteral(literal) if !parent_is_nullish => {
                if literal.value {
                    (entry, St::dead())
                } else {
                    (St::dead(), entry)
                }
            }
            Expression::LogicalExpression(logical) => self.logical_condition(logical, entry),
            // Parentheses and `!` are looked through only around a logical expression; around
            // anything else the whole thing is one opaque condition, as in tsc.
            Expression::ParenthesizedExpression(inner) if is_logical(&inner.expression) => {
                self.condition(&inner.expression, entry, parent_is_nullish)
            }
            Expression::UnaryExpression(unary)
                if unary.operator == UnaryOperator::LogicalNot && is_logical(&unary.argument) =>
            {
                let (when_true, when_false) = self.condition(&unary.argument, entry, false);
                (when_false, when_true)
            }
            other => {
                self.st = entry;
                self.visit_expression(other);
                let after = self.st.clone();
                let (mut when_true, mut when_false) = (after.clone(), after);
                for index in self.removes_undefined(other, true) {
                    when_true.assigned.set(index);
                }
                for index in self.removes_undefined(other, false) {
                    when_false.assigned.set(index);
                }
                (when_true, when_false)
            }
        }
    }

    fn logical_condition<'a>(&mut self, logical: &LogicalExpression<'a>, entry: St) -> (St, St) {
        let nullish = logical.operator == LogicalOperator::Coalesce;
        let left = self.condition(&logical.left, entry, nullish);
        match logical.operator {
            LogicalOperator::And => {
                let right = self.condition(&logical.right, left.0, false);
                (right.0, St::join(&left.1, &right.1))
            }
            // `??` is bound like `||`, with the literal rule off for its operands.
            LogicalOperator::Or | LogicalOperator::Coalesce => {
                let right = self.condition(&logical.right, left.1, nullish);
                (St::join(&left.0, &right.0), right.1)
            }
        }
    }

    // The tracked variables a test proves are not `undefined` on one of its edges, so a read
    // there is no longer unassigned. tsc gets the same from narrowing: a variable that starts as
    // `T | undefined` loses the `undefined` where the test rules it out. Only the plain forms are
    // understood here: `x`, `!x`, `x === undefined` and its negations, `x == null` and its
    // negations, and `typeof x` against a type name. Anything else leaves the variable as it is,
    // which can only report more, never less; the resolver replaces this with real narrowing.
    fn removes_undefined(&self, test: &Expression<'_>, when_true: bool) -> Vec<usize> {
        let mut found = Vec::new();
        self.collect_removes_undefined(test, when_true, &mut found);
        found
    }

    fn collect_removes_undefined(
        &self,
        test: &Expression<'_>,
        when_true: bool,
        out: &mut Vec<usize>,
    ) {
        match test {
            Expression::ParenthesizedExpression(inner) => {
                self.collect_removes_undefined(&inner.expression, when_true, out)
            }
            Expression::UnaryExpression(unary) if unary.operator == UnaryOperator::LogicalNot => {
                self.collect_removes_undefined(&unary.argument, !when_true, out)
            }
            // `x` is truthy, so not `undefined`.
            Expression::Identifier(id) if when_true => out.extend(self.narrowed_index(id)),
            // `isC(x)`: a call to a predicate function narrows its arguments on the true edge.
            Expression::CallExpression(call) if when_true => {
                if let Expression::Identifier(callee) = skip_parens(&call.callee)
                    && let Some(symbol) = self.symbol_of(callee)
                    && self.predicates.contains(&symbol)
                {
                    for argument in &call.arguments {
                        if let Some(Expression::Identifier(id)) =
                            argument.as_expression().map(skip_parens)
                        {
                            out.extend(self.narrowed_index(id));
                        }
                    }
                }
            }
            Expression::BinaryExpression(binary) => {
                use oxc_ast::ast::BinaryOperator as Op;
                // `x instanceof C` and `"k" in x` narrow `x` to the members they admit on the
                // true edge, and `undefined` is none of them.
                if when_true {
                    let operand = match binary.operator {
                        Op::Instanceof => Some(skip_parens(&binary.left)),
                        Op::In => Some(skip_parens(&binary.right)),
                        _ => None,
                    };
                    if let Some(Expression::Identifier(id)) = operand {
                        out.extend(self.narrowed_index(id));
                    }
                }
                let (equal, strict) = match binary.operator {
                    Op::StrictEquality => (true, true),
                    Op::StrictInequality => (false, true),
                    Op::Equality => (true, false),
                    Op::Inequality => (false, false),
                    _ => return,
                };
                // Which edge says "it is the thing compared with".
                let is_it = equal == when_true;
                for (this, other) in [(&binary.left, &binary.right), (&binary.right, &binary.left)]
                {
                    let this = skip_parens(this);
                    let other = skip_parens(other);
                    if let Expression::Identifier(id) = this {
                        if let Some(index) = self.narrowed_index(id) {
                            let compares_undefined = is_undefined(other)
                                || (!strict && matches!(other, Expression::NullLiteral(_)));
                            if compares_undefined {
                                // `x !== undefined`: true edge is "not undefined".
                                if !is_it {
                                    out.push(index);
                                }
                            } else if is_it && strict {
                                // `x === value`: on the true edge x has the value's type. Only a
                                // literal is known not to be `undefined`.
                                if is_literal(other) {
                                    out.push(index);
                                }
                            }
                        }
                    }
                    // `x.constructor === C` and `x.kind === "a"`: a constructor or discriminant
                    // test keeps the members it matches, so on the true edge `undefined` is gone.
                    if is_it
                        && let Expression::StaticMemberExpression(member) = this
                        && let Expression::Identifier(id) = skip_parens(&member.object)
                        && (strict || member.property.name == "constructor")
                        && (member.property.name == "constructor" || is_literal(other))
                    {
                        out.extend(self.narrowed_index(id));
                    }
                    // `typeof x === "string"` and `typeof x !== "undefined"`.
                    if let Expression::UnaryExpression(unary) = this {
                        if unary.operator == UnaryOperator::Typeof {
                            if let (Expression::Identifier(id), Expression::StringLiteral(name)) =
                                (skip_parens(&unary.argument), other)
                            {
                                if let Some(index) = self.narrowed_index(id) {
                                    let names_undefined = name.value == "undefined";
                                    // The true edge of `=== "string"` and the false edge of
                                    // `=== "undefined"` both rule `undefined` out.
                                    if is_it != names_undefined {
                                        out.push(index);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // The tracked variable an identifier names, if it is one and is read in the function it
    // lives in.
    fn tracked_index(&self, id: &IdentifierReference<'_>) -> Option<usize> {
        let tracked = self.tracked.get(&self.symbol_of(id)?)?;
        (tracked.container == self.container).then_some(tracked.index)
    }

    // The index of a tracked variable for narrowing, whichever function declares it: a truthiness
    // test narrows an outer variable inside a closure as it does one of the function's own.
    fn narrowed_index(&self, id: &IdentifierReference<'_>) -> Option<usize> {
        self.tracked
            .get(&self.symbol_of(id)?)
            .map(|tracked| tracked.index)
    }

    fn symbol_of(&self, id: &IdentifierReference<'_>) -> Option<SymbolId> {
        self.scoping
            .get_reference(id.reference_id.get()?)
            .symbol_id()
    }

    // Does anything assign this variable with a plain `x = ...` (or a logical, destructuring or
    // loop assignment), not `x += 1` or `x++`? tsc's `isSymbolAssignedDefinitely`. A variable
    // that nothing assigns that way is "never initialized" and gets no benefit of the doubt in a
    // closure.
    fn has_definite_assignment(&self, symbol: SymbolId) -> bool {
        self.definite.contains(&symbol)
    }

    // Records where an assignment to a tracked variable ends, for the closure rule. An
    // assignment in the function the variable lives in extends over the outermost statement
    // around it that starts after the declaration; one in another function is "past" nothing.
    fn note_assignment(&mut self, id: &IdentifierReference<'_>, definite: bool) {
        if !self.discovering {
            return;
        }
        let Some(symbol) = self.symbol_of(id) else {
            return;
        };
        let Some(tracked) = self.tracked.get(&symbol).copied() else {
            return;
        };
        if definite {
            self.definite.insert(symbol);
        }
        if self.last_assignment.get(&symbol) == Some(&u32::MAX) {
            return;
        }
        let position = if tracked.function == self.function {
            self.statements
                .iter()
                .find(|statement| statement.start > tracked.declaration_start)
                .map_or(id.span.start, |statement| statement.end)
        } else {
            u32::MAX
        };
        self.last_assignment.insert(symbol, position);
    }

    // Inside an async or generator function called where it is written, whose creation point
    // cannot be reached: tsc follows the flow back to that point and finds nothing, so a read
    // there is never an error.
    fn creation_is_dead(&self) -> bool {
        self.frames
            .iter()
            .rev()
            .take_while(|frame| frame.transparent)
            .any(|frame| !frame.creation.live)
    }

    // Is a closure's read of a variable that nothing assigns an error? It is if the variable is
    // a plain `let` with no plain assignment anywhere, and then it depends on where the closure
    // starts its flow. A function declaration, a method or a field initializer starts its own,
    // in which nothing is assigned. An arrow or function expression whose read is past the last
    // assignment continues from where it was created, so it is an error only if the variable was
    // not assigned there; a read before the last assignment starts its own flow too.
    fn closure_read_is_unassigned(
        &mut self,
        symbol: SymbolId,
        tracked: Tracked,
        read: Span,
    ) -> bool {
        if !tracked.mutable_local || self.has_definite_assignment(symbol) {
            return false;
        }
        let past_last = match self.last_assignment.get(&symbol) {
            None => true,
            Some(&position) => position != u32::MAX && position < read.start,
        };
        // Walk outwards through the functions created in the variable's own function. A real
        // closure that cannot continue (or a read before the last assignment) starts a flow in
        // which nothing is assigned. Otherwise the flow continues from where it was created, and
        // a creation point that cannot be reached ends the question: nothing is an error there.
        let mut assigned_there = false;
        let mut reached_declaring_function = false;
        for frame in self.frames.iter().rev() {
            // Past the functions created in the variable's own function, the rest are not part of
            // its flow.
            if reached_declaring_function && frame.parent != tracked.container {
                break;
            }
            if !frame.transparent && (!frame.climbable || !past_last) {
                return true;
            }
            if !frame.creation.live {
                return false;
            }
            assigned_there |= frame.creation.assigned.has(tracked.index);
            if frame.parent == tracked.container {
                reached_declaring_function = true;
            }
        }
        !assigned_there
    }

    // Marks the tracked variables named by an assignment target as assigned.
    fn assign_target(&mut self, target: &AssignmentTarget<'_>, definite: bool) {
        match target {
            AssignmentTarget::AssignmentTargetIdentifier(id) => self.assign(id, definite),
            AssignmentTarget::ArrayAssignmentTarget(array) => {
                for element in array.elements.iter().flatten() {
                    self.assign_maybe_default(element, definite);
                }
                if let Some(rest) = &array.rest {
                    self.assign_target(&rest.target, definite);
                }
            }
            AssignmentTarget::ObjectAssignmentTarget(object) => {
                for property in &object.properties {
                    match property {
                        oxc_ast::ast::AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(p) => {
                            self.assign(&p.binding, definite)
                        }
                        oxc_ast::ast::AssignmentTargetProperty::AssignmentTargetPropertyProperty(p) => {
                            self.assign_maybe_default(&p.binding, definite)
                        }
                    }
                }
                if let Some(rest) = &object.rest {
                    self.assign_target(&rest.target, definite);
                }
            }
            _ => {}
        }
    }

    fn assign_maybe_default(
        &mut self,
        element: &oxc_ast::ast::AssignmentTargetMaybeDefault<'_>,
        definite: bool,
    ) {
        use oxc_ast::ast::AssignmentTargetMaybeDefault as M;
        match element {
            M::AssignmentTargetWithDefault(with_default) => {
                self.assign_target(&with_default.binding, definite)
            }
            other => {
                if let Some(target) = other.as_assignment_target() {
                    self.assign_target(target, definite);
                }
            }
        }
    }

    // An assignment to a variable. Only a definite one makes it assigned afterwards: tsc's flow
    // gives `x += 1` and `x++` the type `x` had before, `undefined` included.
    fn assign(&mut self, id: &IdentifierReference<'_>, definite: bool) {
        self.note_assignment(id, definite);
        if !definite {
            return;
        }
        if let Some(index) = self.tracked_index(id) {
            self.st.assigned.set(index);
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
            self.st = St::dead();
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

impl<'a, 's> Visit<'a> for Analyzer<'s> {
    fn visit_function(
        &mut self,
        function: &oxc_ast::ast::Function<'a>,
        flags: oxc_semantic::ScopeFlags,
    ) {
        let invoked = std::mem::take(&mut self.iife_pending);
        let immediately_invoked = invoked && !function.r#async && !function.generator;
        // The body of a class method, constructor or accessor is checked in place; every other
        // function body is checked later, on its own. See `container`.
        let in_place = self.in_place_function == Some(function.span);
        let kind = Kind {
            climbable: !in_place
                && function.r#type == oxc_ast::ast::FunctionType::FunctionExpression,
            transparent: invoked,
        };
        let outer_function = self.enter_function();
        let method = if in_place { self.method } else { None };
        self.container(immediately_invoked, in_place, kind, |this| {
            let outer = std::mem::take(&mut this.fn_state);
            walk::walk_function(this, function, flags);
            let mine = std::mem::replace(&mut this.fn_state, outer);
            if function.body.is_some() && !this.discovering {
                let error_span = function
                    .return_type
                    .as_ref()
                    .map(|annotation| annotation.span)
                    .or_else(|| method.map(|(name, _)| name))
                    .or_else(|| function.id.as_ref().map(|id| id.span))
                    .unwrap_or(function.span);
                this.out.functions.push(FunctionFact {
                    key: function.span.start,
                    error_span,
                    end_reachable: this.st.live,
                    has_explicit_return: mine.explicit_return,
                    bare_returns: mine.bare_returns,
                    has_annotation: function.return_type.is_some(),
                    returns_a_value: mine.returns_a_value,
                    async_inner: function
                        .r#async
                        .then(|| function.return_type.as_ref())
                        .flatten()
                        .and_then(|annotation| promise_inner(&annotation.type_annotation)),
                    is_async: function.r#async,
                    is_generator: function.generator,
                    is_constructor: method.is_some_and(|(_, constructor)| constructor),
                });
            }
        });
        self.function = outer_function;
    }

    // tsc takes the computed key of a method or accessor, in a class or an object literal, to
    // belong to that member: its flow container is the member, so a variable of the enclosing
    // function or file read there counts as an outer variable and is assumed assigned. Only the
    // key is affected, not the decorators or the body.
    // `a[x]` inside `for (let x in a)`, where `x` is the variable that loop declares: tsc does not
    // report `a` there, though it reports the read in the loop head and any other `a[y]`.
    fn visit_computed_member_expression(
        &mut self,
        member: &oxc_ast::ast::ComputedMemberExpression<'a>,
    ) {
        let symbol_of = |this: &Self, expression: &Expression<'a>| {
            let mut expression = expression;
            while let Expression::ParenthesizedExpression(inner) = expression {
                expression = &inner.expression;
            }
            let Expression::Identifier(id) = expression else {
                return None;
            };
            this.scoping
                .get_reference(id.reference_id.get()?)
                .symbol_id()
        };
        if let (Some(object), Some(key)) = (
            symbol_of(self, &member.object),
            symbol_of(self, &member.expression),
        ) && self.for_in_keys.contains(&(key, object))
        {
            self.visit_expression(&member.expression);
            return;
        }
        walk::walk_computed_member_expression(self, member);
    }

    fn visit_property_key(&mut self, key: &oxc_ast::ast::PropertyKey<'a>) {
        if std::mem::take(&mut self.member_key) {
            self.container(false, true, Kind::OWN_FLOW, |this| {
                walk::walk_property_key(this, key)
            });
        } else {
            walk::walk_property_key(self, key);
        }
    }

    fn visit_object_property(&mut self, property: &oxc_ast::ast::ObjectProperty<'a>) {
        self.member_key = property.computed
            && (property.method || property.kind != oxc_ast::ast::PropertyKind::Init);
        walk::walk_object_property(self, property);
        self.member_key = false;
    }

    fn visit_method_definition(&mut self, method: &oxc_ast::ast::MethodDefinition<'a>) {
        self.member_key = method.computed;
        let saved = self.in_place_function.replace(method.value.span);
        let saved_method = self.method.replace((
            method.key.span(),
            method.kind == oxc_ast::ast::MethodDefinitionKind::Constructor,
        ));
        walk::walk_method_definition(self, method);
        self.member_key = false;
        self.method = saved_method;
        self.in_place_function = saved;
    }

    fn visit_arrow_function_expression(
        &mut self,
        arrow: &oxc_ast::ast::ArrowFunctionExpression<'a>,
    ) {
        let invoked = std::mem::take(&mut self.iife_pending);
        let immediately_invoked = invoked && !arrow.r#async;
        let kind = Kind {
            climbable: true,
            transparent: invoked,
        };
        let outer_function = self.enter_function();
        self.container(immediately_invoked, false, kind, |this| {
            let outer = std::mem::take(&mut this.fn_state);
            walk::walk_arrow_function_expression(this, arrow);
            let mine = std::mem::replace(&mut this.fn_state, outer);
            if matches!(arrow.body, oxc_ast::ast::ArrowFunctionBody::FunctionBody(_))
                && !this.discovering
            {
                this.out.functions.push(FunctionFact {
                    key: arrow.span.start,
                    error_span: arrow
                        .return_type
                        .as_ref()
                        .map_or(arrow.span, |annotation| annotation.span),
                    end_reachable: this.st.live,
                    has_explicit_return: mine.explicit_return,
                    bare_returns: mine.bare_returns,
                    has_annotation: arrow.return_type.is_some(),
                    returns_a_value: mine.returns_a_value,
                    async_inner: arrow
                        .r#async
                        .then(|| arrow.return_type.as_ref())
                        .flatten()
                        .and_then(|annotation| promise_inner(&annotation.type_annotation)),
                    is_async: arrow.r#async,
                    is_generator: false,
                    is_constructor: false,
                });
            }
        });
        self.function = outer_function;
    }

    fn visit_static_block(&mut self, block: &oxc_ast::ast::StaticBlock<'a>) {
        self.container(true, true, Kind::OWN_FLOW, |this| {
            this.visit_statements(&block.body)
        });
    }

    // A namespace body is checked in place, as part of checking the declaration, so inside a
    // namespace that is itself dead nothing is reported again.
    fn visit_ts_module_block(&mut self, block: &oxc_ast::ast::TSModuleBlock<'a>) {
        self.container(false, true, Kind::OWN_FLOW, |this| {
            this.visit_statements(&block.body)
        });
    }

    // A class field initializer is a function of its own for flow purposes.
    fn visit_property_definition(&mut self, property: &oxc_ast::ast::PropertyDefinition<'a>) {
        self.container(false, true, Kind::OWN_FLOW, |this| {
            walk::walk_property_definition(this, property)
        });
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

    // `a && b`, `a || b`, `a ?? b` as a value: the right side runs only on one edge of the left,
    // so what it assigns is not assigned afterwards, and a constant left makes one side dead.
    fn visit_logical_expression(&mut self, logical: &LogicalExpression<'a>) {
        let entry = self.st.clone();
        let (when_true, when_false) = self.logical_condition(logical, entry);
        self.st = St::join(&when_true, &when_false);
    }

    fn visit_conditional_expression(&mut self, conditional: &ConditionalExpression<'a>) {
        let entry = self.st.clone();
        let (when_true, when_false) = self.condition(&conditional.test, entry, false);
        self.st = when_true;
        self.visit_expression(&conditional.consequent);
        let after_then = self.st.clone();
        self.st = when_false;
        self.visit_expression(&conditional.alternate);
        self.st = St::join(&after_then, &self.st);
    }

    // `x = v` assigns after `v` is evaluated; `x += v` reads `x` first; `x ||= v` assigns only
    // when it does not short-circuit.
    fn visit_assignment_expression(&mut self, assignment: &AssignmentExpression<'a>) {
        match assignment.operator {
            AssignmentOperator::Assign => {
                // Targets that are not plain names (`a.b`, `a[i]`) evaluate their parts first.
                self.visit_assignment_target(&assignment.left);
                self.visit_expression(&assignment.right);
                self.assign_target(&assignment.left, true);
            }
            AssignmentOperator::LogicalAnd
            | AssignmentOperator::LogicalOr
            | AssignmentOperator::LogicalNullish => {
                // tsc counts the target of a logical assignment as a definite assignment, not a
                // read, so a variable named there is not reported even when unassigned. Parts of
                // a longer target (`a.b ||= v`) are still evaluated.
                if !matches!(
                    assignment.left,
                    AssignmentTarget::AssignmentTargetIdentifier(_)
                ) {
                    self.visit_assignment_target(&assignment.left);
                }
                // The left side is a condition. On the edge where it short-circuits, `||=` has
                // found it truthy and `??=` has found it not nullish, so it cannot be
                // `undefined` there; `&&=` short-circuits on a falsy value, which can be.
                let mut short_circuit = self.st.clone();
                if assignment.operator != AssignmentOperator::LogicalAnd {
                    if let AssignmentTarget::AssignmentTargetIdentifier(id) = &assignment.left {
                        if let Some(index) = self.tracked_index(id) {
                            short_circuit.assigned.set(index);
                        }
                    }
                }
                self.visit_expression(&assignment.right);
                self.assign_target(&assignment.left, true);
                self.st = St::join(&short_circuit, &self.st);
            }
            _ => {
                self.visit_assignment_target(&assignment.left);
                self.visit_expression(&assignment.right);
                self.assign_target(&assignment.left, false);
            }
        }
    }

    fn visit_update_expression(&mut self, update: &UpdateExpression<'a>) {
        walk::walk_update_expression(self, update);
        if let SimpleAssignmentTarget::AssignmentTargetIdentifier(id) = &update.argument {
            self.assign(id, false);
        }
    }

    // `x!` says the variable is assigned: tsc does not report a read under it.
    fn visit_ts_non_null_expression(&mut self, expression: &oxc_ast::ast::TSNonNullExpression<'a>) {
        if matches!(
            skip_parens(&expression.expression),
            Expression::Identifier(_)
        ) {
            return;
        }
        walk::walk_ts_non_null_expression(self, expression);
    }

    // `export { x }` names a variable without reading its value.
    fn visit_export_specifier(&mut self, _specifier: &oxc_ast::ast::ExportSpecifier<'a>) {}

    fn visit_identifier_reference(&mut self, id: &IdentifierReference<'a>) {
        if self.discovering || !self.st.live {
            return;
        }
        let Some(reference_id) = id.reference_id.get() else {
            return;
        };
        let reference = self.scoping.get_reference(reference_id);
        if !reference.is_read() {
            return;
        }
        let Some(symbol) = reference.symbol_id() else {
            return;
        };
        // `var a = { f: a }`: reading a variable inside its own initializer. Its type is circular,
        // which tsc resolves to `any` (and reports as TS7022), so there is nothing unassigned.
        if self.initializing.contains(&symbol) {
            return;
        }
        let Some(tracked) = self.tracked.get(&symbol).copied() else {
            return;
        };
        let unassigned = if tracked.container == self.container {
            !self.st.assigned.has(tracked.index) && !self.creation_is_dead()
        } else {
            // From inside another function tsc assumes the variable is assigned, unless it is a
            // `let` that nothing assigns with a plain `=`.
            // Narrowed inside this function (`while (cond) { cond; }`): `undefined` is gone.
            self.closure_read_is_unassigned(symbol, tracked, id.span)
                && !self.st.assigned.has(tracked.index)
        };
        if unassigned {
            self.out.unassigned.push(UnassignedRead {
                span: id.span,
                symbol,
                name: id.name.to_string(),
                allows_unassigned: tracked.allows_unassigned,
            });
        }
    }

    fn visit_variable_declaration(&mut self, declaration: &oxc_ast::ast::VariableDeclaration<'a>) {
        let saved = self
            .declaration
            .replace((declaration.kind, declaration.declare));
        walk::walk_variable_declaration(self, declaration);
        self.declaration = saved;
    }

    // A `let` or `var` with a type annotation and no initializer is tracked from here: it is not
    // assigned until something assigns it, and a declaration met again (in a loop body) starts it
    // over.
    fn visit_variable_declarator(&mut self, declarator: &oxc_ast::ast::VariableDeclarator<'a>) {
        let binding_symbol = match &declarator.id {
            BindingPattern::BindingIdentifier(binding) => binding.symbol_id.get(),
            _ => None,
        };
        if let Some(init) = &declarator.init {
            self.initializing.extend(binding_symbol);
            self.visit_expression(init);
            if binding_symbol.is_some() {
                self.initializing.pop();
            }
        }
        self.visit_binding_pattern(&declarator.id);
        let Some((kind, declare)) = self.declaration else {
            return;
        };
        if let Some(symbol) = binding_symbol {
            if declare {
                self.ambient_vars.insert(symbol);
            } else if self.ambient_vars.contains(&symbol) {
                return;
            }
        }
        // A `var` with an initializer is tracked too: the declaration assigns it, but a path that
        // skips the declaration (a loop that may run zero times, an untaken branch) reaches a
        // later read with it unassigned, and tsc reports that against the inferred type.
        let trackable = !declarator.definite
            && !declare
            && !self.in_for_head
            && match kind {
                VariableDeclarationKind::Let => {
                    declarator.init.is_none() && declarator.type_annotation.is_some()
                }
                // `var x = null` has an evolving type in tsc, not the type `null`, and is not
                // reported.
                VariableDeclarationKind::Var => {
                    declarator.type_annotation.is_some()
                        || declarator
                            .init
                            .as_ref()
                            .is_some_and(|init| !matches!(init, Expression::NullLiteral(_)))
                }
                _ => false,
            };
        if !trackable {
            // `var m: T; var m = value;` declares `m` twice: the second declaration assigns it.
            // So does the head of a `for (var m in ...)` over a variable declared elsewhere.
            if (declarator.init.is_some() || self.in_for_head)
                && let BindingPattern::BindingIdentifier(binding) = &declarator.id
                && let Some(symbol) = binding.symbol_id.get()
                && let Some(tracked) = self.tracked.get(&symbol).copied()
                && tracked.container == self.container
            {
                self.st.assigned.set(tracked.index);
            }
            return;
        }
        let BindingPattern::BindingIdentifier(binding) = &declarator.id else {
            return;
        };
        let Some(symbol) = binding.symbol_id.get() else {
            return;
        };
        let next = self.tracked.len();
        let container = self.container;
        // Not a "mutable local" when exported, or when it sits at the top of a script, where it
        // is a global.
        let mutable_local = kind == VariableDeclarationKind::Let
            && !self.in_export
            && !(self.is_script && container == 0 && self.statements.len() <= 1);
        let tracked = *self.tracked.entry(symbol).or_insert(Tracked {
            index: next,
            container,
            declaration_start: declarator.span.start,
            function: self.function,
            mutable_local,
            allows_unassigned: match (&declarator.type_annotation, &declarator.init) {
                (Some(annotation), _) => annotation_allows_unassigned(
                    &annotation.type_annotation,
                    self.scoping,
                    &self.namespaces,
                ),
                (None, Some(init)) => initializer_excludes_undefined(init).then_some(false),
                (None, None) => None,
            },
        });
        if declarator.init.is_some() {
            self.st.assigned.set(tracked.index);
        } else {
            self.st.assigned.clear(tracked.index);
        }
    }

    // The one hook every statement list goes through. Walks the list in order and groups the
    // dead statements the way tsc reports them: one diagnostic for each maximal run of adjacent
    // dead statements that can execute, ended by anything that cannot (a function declaration,
    // an empty statement, a `var` without initializer, a block, an import) or by a live one.
    fn visit_statements(&mut self, statements: &oxc_allocator::Vec<'a, Statement<'a>>) {
        let mut run: Option<Span> = None;
        for statement in statements {
            let dead = !self.st.live;
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
        let entry = self.st.clone();
        let pending = std::mem::take(&mut self.pending_labels);
        if !entry.live {
            self.out.dead.insert(statement.span().start);
        }
        let extends_assignment = extends_assignment_position(statement);
        if extends_assignment {
            self.statements.push(statement.span());
        }
        match statement {
            Statement::BlockStatement(block) => self.visit_statements(&block.body),
            Statement::ReturnStatement(statement) => {
                // The binder only meets a `return` it can reach; the checker looks at every one.
                if entry.live {
                    self.fn_state.explicit_return = true;
                }
                match &statement.argument {
                    None => self.fn_state.bare_returns.push(statement.span),
                    Some(argument) if !is_undefined(skip_parens(argument)) => {
                        self.fn_state.returns_a_value = true;
                    }
                    Some(_) => {}
                }
                if let Some(argument) = &statement.argument {
                    self.visit_expression(argument);
                }
                if let Some(target) = self.return_target {
                    self.arrive(target);
                }
                self.st = St::dead();
            }
            Statement::ThrowStatement(statement) => {
                self.visit_expression(&statement.argument);
                self.st = St::dead();
            }
            Statement::BreakStatement(statement) => {
                self.jump(statement.label.as_ref().map(|l| l.name.as_str()), true);
            }
            Statement::ContinueStatement(statement) => {
                self.jump(statement.label.as_ref().map(|l| l.name.as_str()), false);
            }
            Statement::IfStatement(statement) => {
                let (when_true, when_false) = self.condition(&statement.test, entry, false);
                self.st = when_true;
                self.child(&statement.consequent);
                let after_then = self.st.clone();
                self.st = when_false;
                if let Some(alternate) = &statement.alternate {
                    self.child(alternate);
                }
                self.st = St::join(&after_then, &self.st);
            }
            Statement::WhileStatement(statement) => {
                let (when_true, when_false) = self.condition(&statement.test, entry, false);
                let post = self.new_target();
                let again = self.new_target();
                self.st = when_true;
                self.in_loop(pending, post, again, |this| this.child(&statement.body));
                self.st = St::join(&when_false, &self.at(post));
            }
            Statement::DoWhileStatement(statement) => {
                let post = self.new_target();
                let before_test = self.new_target();
                self.st = entry;
                self.in_loop(pending, post, before_test, |this| {
                    this.child(&statement.body);
                    this.arrive(before_test);
                });
                let test_entry = self.at(before_test);
                let (_, when_false) = self.condition(&statement.test, test_entry, false);
                self.st = St::join(&when_false, &self.at(post));
            }
            Statement::ForStatement(statement) => {
                match &statement.init {
                    Some(ForStatementInit::VariableDeclaration(declaration)) => {
                        self.visit_variable_declaration(declaration)
                    }
                    Some(init) => walk::walk_for_statement_init(self, init),
                    None => {}
                }
                let test_entry = self.st.clone();
                let (when_true, when_false) = match &statement.test {
                    // No condition: the true edge is the flow, the false edge is closed.
                    None => (test_entry, St::dead()),
                    Some(test) => self.condition(test, test_entry, false),
                };
                let post = self.new_target();
                let increment = self.new_target();
                self.st = when_true;
                self.in_loop(pending, post, increment, |this| {
                    this.child(&statement.body);
                    this.arrive(increment);
                });
                self.st = self.at(increment);
                if let Some(update) = &statement.update {
                    self.visit_expression(update);
                }
                self.st = St::join(&when_false, &self.at(post));
            }
            Statement::ForInStatement(statement) => {
                self.visit_expression(&statement.right);
                let key = self.for_in_key(statement);
                self.for_in_keys.extend(key);
                self.for_in_or_of(&statement.left, pending, &statement.body);
                if key.is_some() {
                    self.for_in_keys.pop();
                }
            }
            Statement::ForOfStatement(statement) => {
                self.visit_expression(&statement.right);
                self.for_in_or_of(&statement.left, pending, &statement.body);
            }
            Statement::LabeledStatement(statement) => self.labeled(statement, pending),
            Statement::SwitchStatement(statement) => self.switch(statement),
            Statement::TryStatement(statement) => self.try_statement(statement, entry),
            Statement::ExportDeclaration(_) => {
                let saved = std::mem::replace(&mut self.in_export, true);
                walk::walk_statement(self, statement);
                self.in_export = saved;
            }
            other => walk::walk_statement(self, other),
        }
        if extends_assignment {
            self.statements.pop();
        }
    }
}

impl<'a, 's> Analyzer<'s> {
    // `for (let x in a)` or `for (var x in a)`: the symbols of `x` and of `a`.
    fn for_in_key(
        &self,
        statement: &oxc_ast::ast::ForInStatement<'a>,
    ) -> Option<(SymbolId, SymbolId)> {
        let ForStatementLeft::VariableDeclaration(declaration) = &statement.left else {
            return None;
        };
        let [declarator] = declaration.declarations.as_slice() else {
            return None;
        };
        let BindingPattern::BindingIdentifier(binding) = &declarator.id else {
            return None;
        };
        let mut right = &statement.right;
        while let Expression::ParenthesizedExpression(inner) = right {
            right = &inner.expression;
        }
        let Expression::Identifier(object) = right else {
            return None;
        };
        let object = self
            .scoping
            .get_reference(object.reference_id.get()?)
            .symbol_id()?;
        Some((binding.symbol_id.get()?, object))
    }

    // After a `for...in` or `for...of` the flow is reachable whenever the loop was entered: the
    // loop label feeds the exit directly, whatever the body does. The loop assigns its variable
    // at the top of every pass, but not on the way out.
    fn for_in_or_of(&mut self, left: &ForStatementLeft<'a>, pending: usize, body: &Statement<'a>) {
        let head = self.st.clone();
        let saved_head = std::mem::replace(&mut self.in_for_head, true);
        walk::walk_for_statement_left(self, left);
        self.in_for_head = saved_head;
        if let Some(target) = left.as_assignment_target() {
            self.assign_target(target, true);
        }
        let body_entry = self.st.clone();
        let post = self.new_target();
        let again = self.new_target();
        self.st = head;
        self.arrive(post);
        self.st = body_entry;
        self.in_loop(pending, post, again, |this| this.child(body));
        self.st = self.at(post);
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
        self.st = self.at(post);
    }

    // A `switch`. Every clause is entered from the top of the switch, so each is live whenever
    // the switch is, with what was assigned before it and nothing a clause above it assigned:
    // the top of the switch is one of the ways in, and assignments only add. Clauses fall into
    // one another. The exit is reached by a `break`, by the end of the last clause, and, with no
    // `default`, by "no clause matched".
    fn switch(&mut self, statement: &SwitchStatement<'a>) {
        self.visit_expression(&statement.discriminant);
        let top = self.st.clone();
        let post = self.new_target();
        let saved = self.jumps.clone();
        self.jumps.break_target = Some(post);
        let mut end = top.clone();
        for (index, case) in statement.cases.iter().enumerate() {
            self.st = top.clone();
            if let Some(test) = &case.test {
                self.visit_expression(test);
                self.st = top.clone();
            }
            self.visit_statements(&case.consequent);
            end = self.st.clone();
            if self.st.live && !case.consequent.is_empty() && index + 1 != statement.cases.len() {
                self.out.fallthroughs.push(case.span);
            }
        }
        self.st = end;
        self.arrive(post);
        if !statement.cases.iter().any(|case| case.test.is_none()) {
            self.st = top;
            self.arrive(post);
        }
        self.jumps = saved;
        self.st = self.at(post);
    }

    // A `try`. The `catch` and the `finally` are entered from the start of the `try`, so both
    // are live whenever it is, and neither has anything the `try` assigned. What follows is live
    // when the `finally` can finish and the `try` or the `catch` can finish normally, and has
    // what both of those assigned plus what the `finally` assigned.
    fn try_statement(&mut self, statement: &TryStatement<'a>, entry: St) {
        // With a `finally`, a `return` in the `try` or the `catch` goes to the `finally` first
        // and on to the outer return target only if the `finally` can finish.
        let outer_return = self.return_target;
        let through_finally = statement.finalizer.as_ref().map(|_| self.new_target());
        if through_finally.is_some() {
            self.return_target = through_finally;
        }
        self.st = entry.clone();
        self.visit_statements(&statement.block.body);
        let mut normal = self.st.clone();
        if let Some(handler) = &statement.handler {
            self.st = entry.clone();
            if let Some(param) = &handler.param {
                self.visit_binding_pattern_exprs(&param.pattern);
            }
            self.visit_statements(&handler.body.body);
            normal = St::join(&normal, &self.st);
        }
        self.return_target = outer_return;
        match &statement.finalizer {
            Some(finalizer) => {
                self.st = entry;
                self.visit_statements(&finalizer.body);
                let finished = self.st.clone();
                if let (Some(outer), Some(inner)) = (outer_return, through_finally) {
                    if finished.live {
                        if let Some(bits) = self.arrivals[inner].clone() {
                            let mut bits = bits;
                            bits.union(&finished.assigned);
                            self.arrive_with(outer, &bits);
                        }
                    }
                }
                self.st = if finished.live && normal.live {
                    let mut after = normal;
                    after.assigned.union(&finished.assigned);
                    after
                } else {
                    St::dead()
                };
            }
            None => self.st = normal,
        }
    }
}

// Does this annotation, read on its own, let a variable be read before it is assigned? tsc's
// `assumeInitialized` for the declared type: `any`, `unknown`, `void` and `undefined` do, and so
// does a union with one of them. A keyword type or a literal shape does not. A name could be an
// alias for anything, so nothing is said about it.
fn annotation_allows_unassigned(
    annotation: &oxc_ast::ast::TSType<'_>,
    scoping: &Scoping,
    namespaces: &NamespaceMembers,
) -> Option<bool> {
    use oxc_ast::ast::TSType;
    match annotation {
        TSType::TSAnyKeyword(_)
        | TSType::TSUnknownKeyword(_)
        | TSType::TSVoidKeyword(_)
        | TSType::TSUndefinedKeyword(_) => Some(true),
        TSType::TSNumberKeyword(_)
        | TSType::TSStringKeyword(_)
        | TSType::TSBooleanKeyword(_)
        | TSType::TSBigIntKeyword(_)
        | TSType::TSSymbolKeyword(_)
        | TSType::TSObjectKeyword(_)
        | TSType::TSNullKeyword(_)
        | TSType::TSNeverKeyword(_)
        | TSType::TSLiteralType(_)
        | TSType::TSTypeLiteral(_)
        | TSType::TSArrayType(_)
        | TSType::TSTupleType(_)
        | TSType::TSFunctionType(_) => Some(false),
        // A library object type nothing makes nullable (a name a program does not shadow).
        TSType::TSTypeReference(reference)
            if matches!(
                &reference.type_name,
                oxc_ast::ast::TSTypeName::IdentifierReference(id)
                    if matches!(
                        id.name.as_str(),
                        "String" | "Number" | "Boolean" | "Symbol" | "BigInt" | "Object"
                            | "Function" | "Date" | "RegExp" | "Error" | "Array" | "Map" | "Set"
                            | "WeakMap" | "WeakSet" | "Promise" | "ReadonlyArray"
                    )
            ) =>
        {
            Some(false)
        }
        // A class, interface or enum declared in the program: an instance of one is never
        // `undefined`, whatever its members are, so the checker need not understand the shape
        // for the read to be an error. A type alias or type parameter could stand for a union
        // or `any` and still needs the checker; so does a name that is only imported.
        TSType::TSTypeReference(reference) => {
            let id = match &reference.type_name {
                oxc_ast::ast::TSTypeName::IdentifierReference(id) => id,
                oxc_ast::ast::TSTypeName::QualifiedName(qualified) => {
                    return qualified_name_is_non_nullable(qualified, scoping, namespaces);
                }
                _ => return None,
            };
            let symbol = scoping.get_reference(id.reference_id.get()?).symbol_id()?;
            let flags = scoping.symbol_flags(symbol);
            let declared_here = flags.intersects(
                oxc_semantic::SymbolFlags::Class
                    | oxc_semantic::SymbolFlags::Interface
                    | oxc_semantic::SymbolFlags::Enum,
            );
            let could_be_anything = flags.intersects(
                oxc_semantic::SymbolFlags::TypeAlias
                    | oxc_semantic::SymbolFlags::TypeParameter
                    | oxc_semantic::SymbolFlags::Import,
            );
            (declared_here && !could_be_anything).then_some(false)
        }
        TSType::TSUnionType(union) => {
            let mut all_known = true;
            for member in &union.types {
                match annotation_allows_unassigned(member, scoping, namespaces) {
                    Some(true) => return Some(true),
                    Some(false) => {}
                    None => all_known = false,
                }
            }
            all_known.then_some(false)
        }
        _ => None,
    }
}

/// What a namespace exports that a type annotation can name: a class, interface or enum (never
/// nullable), or another namespace to keep walking into.
#[derive(Debug, Clone, Copy)]
enum NamespaceMember {
    NonNullable,
    Namespace(SymbolId),
}

/// For each namespace symbol, its exported members by name. Merged declarations of one namespace
/// share a symbol, so they add to the same table.
type NamespaceMembers = HashMap<SymbolId, HashMap<String, NamespaceMember>>;

fn collect_namespaces(statements: &[Statement<'_>], members: &mut NamespaceMembers) {
    for statement in statements {
        let declaration = match statement {
            Statement::ExportDeclaration(export) => Some(&export.declaration),
            other => other.as_declaration(),
        };
        if let Some(Declaration::TSNamespaceDeclaration(namespace)) = declaration {
            collect_namespace(namespace, members);
        }
    }
}

fn collect_namespace(namespace: &TSNamespaceDeclaration<'_>, members: &mut NamespaceMembers) {
    let Some(symbol) = namespace.id.symbol_id.get() else {
        return;
    };
    match &namespace.body {
        // `namespace a.b {}`: `b` is an export of `a`.
        TSNamespaceDeclarationBody::TSNamespaceDeclaration(inner) => {
            if let Some(inner_symbol) = inner.id.symbol_id.get() {
                members.entry(symbol).or_default().insert(
                    inner.id.name.to_string(),
                    NamespaceMember::Namespace(inner_symbol),
                );
            }
            collect_namespace(inner, members);
        }
        TSNamespaceDeclarationBody::TSModuleBlock(block) => {
            for statement in &block.body {
                // Everything in an ambient namespace is exported; elsewhere only what says so.
                let (exported, declaration) = match statement {
                    Statement::ExportDeclaration(export) => (true, Some(&export.declaration)),
                    other => (namespace.declare, other.as_declaration()),
                };
                let entry = match declaration {
                    Some(Declaration::ClassDeclaration(class)) => class
                        .id
                        .as_ref()
                        .map(|id| (id.name.to_string(), NamespaceMember::NonNullable)),
                    Some(Declaration::TSInterfaceDeclaration(interface)) => {
                        Some((interface.id.name.to_string(), NamespaceMember::NonNullable))
                    }
                    Some(Declaration::TSEnumDeclaration(e)) => {
                        Some((e.id.name.to_string(), NamespaceMember::NonNullable))
                    }
                    Some(Declaration::TSNamespaceDeclaration(inner)) => {
                        collect_namespace(inner, members);
                        inner
                            .id
                            .symbol_id
                            .get()
                            .map(|s| (inner.id.name.to_string(), NamespaceMember::Namespace(s)))
                    }
                    _ => None,
                };
                if let (true, Some((name, member))) = (exported, entry) {
                    members.entry(symbol).or_default().insert(name, member);
                }
            }
        }
    }
}

// `a.b.C` in an annotation: `a` is a namespace of this program, each step is an exported
// namespace, and `C` is an exported class, interface or enum. Anything else is left to the checker.
fn qualified_name_is_non_nullable(
    qualified: &oxc_ast::ast::TSQualifiedName<'_>,
    scoping: &Scoping,
    namespaces: &NamespaceMembers,
) -> Option<bool> {
    let mut path = vec![qualified.right.name.as_str()];
    let mut current = &qualified.left;
    let root = loop {
        match current {
            oxc_ast::ast::TSTypeName::QualifiedName(inner) => {
                path.push(inner.right.name.as_str());
                current = &inner.left;
            }
            oxc_ast::ast::TSTypeName::IdentifierReference(id) => break id,
            oxc_ast::ast::TSTypeName::ThisExpression(_) => return None,
        }
    };
    path.reverse();
    let mut symbol = scoping
        .get_reference(root.reference_id.get()?)
        .symbol_id()?;
    let flags = scoping.symbol_flags(symbol);
    if !flags.intersects(
        oxc_semantic::SymbolFlags::NamespaceModule | oxc_semantic::SymbolFlags::ValueModule,
    ) || flags
        .intersects(oxc_semantic::SymbolFlags::Import | oxc_semantic::SymbolFlags::TypeImport)
    {
        return None;
    }
    for (index, segment) in path.iter().enumerate() {
        let member = namespaces.get(&symbol)?.get(*segment)?;
        let last = index + 1 == path.len();
        match (member, last) {
            (NamespaceMember::NonNullable, true) => return Some(false),
            (NamespaceMember::Namespace(next), false) => symbol = *next,
            _ => return None,
        }
    }
    None
}

// An initializer whose value is never `undefined`, so a variable declared by it has a type that
// does not allow `undefined` even where the checker has no type for it: a literal, an object,
// an array, a function, a class, a `new`. Anything else (a name, a call) says nothing here.
fn initializer_excludes_undefined(init: &Expression<'_>) -> bool {
    match init {
        Expression::ParenthesizedExpression(inner) => {
            initializer_excludes_undefined(&inner.expression)
        }
        Expression::NumericLiteral(_)
        | Expression::StringLiteral(_)
        | Expression::BooleanLiteral(_)
        | Expression::BigIntLiteral(_)
        | Expression::TemplateLiteral(_)
        | Expression::ObjectExpression(_)
        | Expression::ArrayExpression(_)
        | Expression::FunctionExpression(_)
        | Expression::ArrowFunctionExpression(_)
        | Expression::ClassExpression(_)
        | Expression::NewExpression(_) => true,
        _ => false,
    }
}

// The statements tsc extends the position of an assignment over: its `extendAssignmentPosition`.
fn extends_assignment_position(statement: &Statement<'_>) -> bool {
    matches!(
        statement,
        Statement::VariableDeclaration(_)
            | Statement::ExpressionStatement(_)
            | Statement::IfStatement(_)
            | Statement::DoWhileStatement(_)
            | Statement::WhileStatement(_)
            | Statement::ForStatement(_)
            | Statement::ForInStatement(_)
            | Statement::ForOfStatement(_)
            | Statement::WithStatement(_)
            | Statement::SwitchStatement(_)
            | Statement::TryStatement(_)
            | Statement::ClassDeclaration(_)
    )
}

fn skip_parens<'e, 'a>(expression: &'e Expression<'a>) -> &'e Expression<'a> {
    let mut current = expression;
    while let Expression::ParenthesizedExpression(inner) = current {
        current = &inner.expression;
    }
    current
}

fn is_undefined(expression: &Expression<'_>) -> bool {
    match expression {
        Expression::Identifier(id) => id.name == "undefined",
        Expression::UnaryExpression(unary) => {
            unary.operator == UnaryOperator::Void
                && matches!(unary.argument, Expression::NumericLiteral(_))
        }
        _ => false,
    }
}

fn is_literal(expression: &Expression<'_>) -> bool {
    matches!(
        expression,
        Expression::StringLiteral(_)
            | Expression::NumericLiteral(_)
            | Expression::BooleanLiteral(_)
            | Expression::NullLiteral(_)
            | Expression::BigIntLiteral(_)
    )
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
