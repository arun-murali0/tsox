// Owns: the narrowing overlay (`NarrowState`), jump frames for break/continue, the write map,
// and the join of two overlays. Does not decide what a test narrows: see `condition.rs`.

use super::*;

// A narrow overlay on top of the declared symbol types in CheckContext.symbols: a
// small association list, not a full map, since only a handful of variables are
// ever narrowed within one branch. Looked up before falling back to a symbol's
// declared type wherever narrowing might apply.
//
// Besides the per-symbol entries there are per-property-path entries (`user.address`,
// `a.b.c`), kept in their own list so the symbol API below is unchanged: a path entry is
// only ever read through get_path and never shadows a symbol's own entry.
#[derive(Default, Clone)]
pub struct NarrowState(pub(super) Vec<(SymbolId, TypeId)>, Vec<PathEntry>);

#[derive(Clone)]
pub(super) struct PathEntry {
    symbol_id: SymbolId,
    path: Vec<String>,
    type_id: TypeId,
}

impl NarrowState {
    pub fn new() -> Self {
        Self(Vec::new(), Vec::new())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty() && self.1.is_empty()
    }

    /// A state holding just one variable at one type.
    pub fn single(symbol_id: SymbolId, type_id: TypeId) -> Self {
        Self::from_single(symbol_id, type_id)
    }

    pub(super) fn from_single(symbol_id: SymbolId, type_id: TypeId) -> Self {
        Self(vec![(symbol_id, type_id)], Vec::new())
    }

    pub(super) fn from_path(symbol_id: SymbolId, path: Vec<String>, type_id: TypeId) -> Self {
        Self(
            Vec::new(),
            vec![PathEntry {
                symbol_id,
                path,
                type_id,
            }],
        )
    }

    pub fn get(&self, symbol_id: SymbolId) -> Option<TypeId> {
        self.0
            .iter()
            .find(|&&(id, _)| id == symbol_id)
            .map(|&(_, ty)| ty)
    }

    pub fn insert(&mut self, symbol_id: SymbolId, type_id: TypeId) {
        if let Some(entry) = self.0.iter_mut().find(|(id, _)| *id == symbol_id) {
            entry.1 = type_id;
        } else {
            self.0.push((symbol_id, type_id));
        }
    }

    // Keeps the symbol entries `keep` accepts and drops every path entry.
    pub(super) fn retain_symbols(&mut self, mut keep: impl FnMut(SymbolId) -> bool) {
        self.0.retain(|&(symbol_id, _)| keep(symbol_id));
        self.1.clear();
    }

    pub fn extend(&mut self, other: NarrowState) {
        for (symbol_id, type_id) in other.0 {
            self.insert(symbol_id, type_id);
        }
        for entry in other.1 {
            self.insert_path(entry.symbol_id, entry.path, entry.type_id);
        }
    }

    // What `a.b.c` is narrowed to, where `path` is `["b", "c"]` and `symbol_id` is `a`.
    pub fn get_path(&self, symbol_id: SymbolId, path: &[String]) -> Option<TypeId> {
        self.1
            .iter()
            .find(|entry| entry.symbol_id == symbol_id && entry.path == path)
            .map(|entry| entry.type_id)
    }

    pub fn insert_path(&mut self, symbol_id: SymbolId, path: Vec<String>, type_id: TypeId) {
        if let Some(entry) = self
            .1
            .iter_mut()
            .find(|entry| entry.symbol_id == symbol_id && entry.path == path)
        {
            entry.type_id = type_id;
        } else {
            self.1.push(PathEntry {
                symbol_id,
                path,
                type_id,
            });
        }
    }

    pub fn has_paths(&self) -> bool {
        !self.1.is_empty()
    }

    // Whether anything is recorded about this symbol, its own entry or one of its paths.
    pub fn mentions(&self, symbol_id: SymbolId) -> bool {
        self.get(symbol_id).is_some() || self.1.iter().any(|entry| entry.symbol_id == symbol_id)
    }

    // Forgets everything known about a symbol: its own entry and every path under it.
    pub fn remove(&mut self, symbol_id: SymbolId) {
        self.0.retain(|&(id, _)| id != symbol_id);
        self.remove_paths_of(symbol_id);
    }

    // Forgets the paths under a symbol but keeps its own entry: what an assignment to
    // the symbol itself does to `x.a`, `x.a.b`, since those described the old value.
    pub fn remove_paths_of(&mut self, symbol_id: SymbolId) {
        self.1.retain(|entry| entry.symbol_id != symbol_id);
    }

    // Forgets `path` and every path below it: assigning `user.address` ends what was
    // known about `user.address` and `user.address.city` alike.
    pub fn remove_path_prefix(&mut self, symbol_id: SymbolId, path: &[String]) {
        self.1
            .retain(|entry| !(entry.symbol_id == symbol_id && entry.path.starts_with(path)));
    }

    // The part of this state a closure may keep seeing: symbol entries `keep` accepts, and
    // no property paths at all, since a property can be reassigned by anything that runs
    // between the closure being made and called.
    pub fn for_closure(&self, keep: impl Fn(SymbolId) -> bool) -> NarrowState {
        NarrowState(
            self.0
                .iter()
                .filter(|&&(id, _)| keep(id))
                .copied()
                .collect(),
            Vec::new(),
        )
    }

    // Whether two states record the same types. Order does not matter. Used to see that a
    // loop's head state has stopped changing.
    pub fn same_as(&self, other: &NarrowState) -> bool {
        self.0.len() == other.0.len()
            && self.1.len() == other.1.len()
            && self.0.iter().all(|&(id, ty)| other.get(id) == Some(ty))
            && self
                .1
                .iter()
                .all(|entry| other.get_path(entry.symbol_id, &entry.path) == Some(entry.type_id))
    }
}

// One enclosing construct a `break` or `continue` can jump to. While the walker is inside
// it, a jump records the narrowing in force at that point here, so the code after the
// construct (or the next pass of a loop) can join it in. See control_flow.rs.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FrameKind {
    Loop,
    Switch,
    // `label: { ... }`, a target for `break label` only.
    Label,
}

pub struct JumpFrame {
    pub label: Option<String>,
    pub kind: FrameKind,
    pub breaks: Vec<NarrowState>,
    pub continues: Vec<NarrowState>,
}

impl JumpFrame {
    pub fn new(label: Option<String>, kind: FrameKind) -> Self {
        Self {
            label,
            kind,
            breaks: Vec::new(),
            continues: Vec::new(),
        }
    }
}

// Where in the source each variable is written, found once per file before checking. Two
// questions are asked of it: which variables does a loop (or try block) assign, so its
// head state can be settled, and is a variable assigned after a given point, which decides
// whether a closure may keep an outer narrowing.
//
// `member` records `a.b = v` against `a`, because that write does not change `a` itself
// but does end what is known about its properties.
#[derive(Default)]
pub struct WriteMap {
    direct: HashMap<SymbolId, Vec<u32>>,
    member: HashMap<SymbolId, Vec<u32>>,
}

impl WriteMap {
    pub fn collect(program: &Program, scoping: &Scoping) -> Self {
        let mut collector = WriteCollector {
            scoping,
            map: WriteMap::default(),
        };
        collector.visit_program(program);
        collector.map
    }

    // Every symbol written, directly or through a property, between the two offsets.
    pub fn written_in(&self, start: u32, end: u32) -> Vec<SymbolId> {
        let mut found = Vec::new();
        for map in [&self.direct, &self.member] {
            for (&symbol_id, positions) in map {
                if positions.iter().any(|&p| p >= start && p < end) && !found.contains(&symbol_id) {
                    found.push(symbol_id);
                }
            }
        }
        found
    }

    // A direct write to the symbol at or after `position`.
    pub fn assigned_at_or_after(&self, symbol_id: SymbolId, position: u32) -> bool {
        self.direct
            .get(&symbol_id)
            .is_some_and(|positions| positions.iter().any(|&p| p >= position))
    }
}

pub(super) struct WriteCollector<'s> {
    scoping: &'s Scoping,
    map: WriteMap,
}

impl<'a> Visit<'a> for WriteCollector<'_> {
    // Every assignment, update, destructuring target and for-in/of head reaches here as an
    // identifier reference the semantic pass has flagged as a write.
    fn visit_identifier_reference(&mut self, it: &IdentifierReference<'a>) {
        let Some(reference_id) = it.reference_id.get() else {
            return;
        };
        let reference = self.scoping.get_reference(reference_id);
        if reference.is_write()
            && let Some(symbol_id) = reference.symbol_id()
        {
            self.map
                .direct
                .entry(symbol_id)
                .or_default()
                .push(it.span.start);
        }
    }

    fn visit_assignment_expression(&mut self, it: &AssignmentExpression<'a>) {
        if let AssignmentTarget::StaticMemberExpression(member) = &it.left
            && let Some(target) = path_of_member(member, self.scoping)
        {
            self.map
                .member
                .entry(target.symbol_id)
                .or_default()
                .push(it.span.start);
        }
        walk::walk_assignment_expression(self, it);
    }

    fn visit_update_expression(&mut self, it: &UpdateExpression<'a>) {
        // `o.count++` writes the property the way `o.count = o.count + 1` does, so the
        // loop and try walks must count it as a write too. A computed or a destructured
        // write is left out on purpose: the forward walk resets no narrowing for those
        // shapes, so head passes triggered for them would walk again only to conclude
        // the same thing.
        if let SimpleAssignmentTarget::StaticMemberExpression(member) = &it.argument
            && let Some(target) = path_of_member(member, self.scoping)
        {
            self.map
                .member
                .entry(target.symbol_id)
                .or_default()
                .push(it.span.start);
        }
        walk::walk_update_expression(self, it);
    }
}

// The state after two control-flow paths meet: a symbol narrowed on both paths
// becomes the union of the two narrowed types; a symbol narrowed on only one path
// is dropped, since the other path still holds whatever the symbol had before and
// the union of that with the narrowed type is no narrower than the earlier state.
// Used where paths rejoin (after an if/else, after a loop, for the true side of
// `a || b` and the false side of `a && b`).
pub fn join_states(a: &NarrowState, b: &NarrowState, arena: &mut TypeArena) -> NarrowState {
    let mut joined = NarrowState::new();
    for &(symbol_id, a_type) in &a.0 {
        if let Some(b_type) = b.get(symbol_id) {
            joined.insert(symbol_id, arena.alloc_union(vec![a_type, b_type]));
        }
    }
    for entry in &a.1 {
        if let Some(b_type) = b.get_path(entry.symbol_id, &entry.path) {
            joined.insert_path(
                entry.symbol_id,
                entry.path.clone(),
                arena.alloc_union(vec![entry.type_id, b_type]),
            );
        }
    }
    joined
}

// Given a condition expression, returns the type overlay that should apply in the
// true branch and the one that should apply in the false branch. Recognizes a
// handful of specific shapes (typeof checks, equality against null, undefined, or
// a literal value, a bare identifier's own truthiness, and `!` or parentheses
// wrapping any of those); anything else narrows nothing in either branch, which
// is always safe, just less precise.
// After `x = value`, x reads as the members of its declared union that the assigned type can be
// assigned to (tsc's assignment narrowing), or as the declared type when that leaves nothing or the
// declared type is not a union. The one rule for it: the forward pass applies it where it assigns,
// and the flow resolver applies it at an assignment node.
pub fn assigned_narrowed_type(
    ctx: &mut CheckContext<'_, '_>,
    declared: TypeId,
    assigned: TypeId,
) -> TypeId {
    let members = match ctx.arena.get(declared) {
        Type::Union(members) => members.clone(),
        _ => return declared,
    };
    // While a loop is only being probed for its head state, a value that came out of reading
    // a variable the probe has not finished narrowing (`x = x.slice()` where `x` is still
    // `never`) is an `any` that says nothing; tsc leaves such a type out as incomplete.
    if ctx.loop_probe && matches!(ctx.arena.get(assigned), Type::Any | Type::Error) {
        return ctx.arena.never();
    }
    let mut kept = Vec::with_capacity(members.len());
    for member in members {
        if ctx.semantic().is_assignable(assigned, member) {
            kept.push(member);
        }
    }
    if kept.is_empty() {
        declared
    } else {
        ctx.arena.alloc_union(kept)
    }
}
