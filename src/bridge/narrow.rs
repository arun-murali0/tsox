use std::collections::HashMap;

use oxc_ast::ast::{
    AssignmentExpression, AssignmentTarget, BinaryExpression, BinaryOperator, Expression,
    IdentifierReference, LogicalOperator, Program, StaticMemberExpression, UnaryOperator,
};
use oxc_ast_visit::{Visit, walk};
use oxc_semantic::{Scoping, SymbolId};

use crate::arena::{TypeArena, TypeId};
use crate::bridge::context::CheckContext;
use crate::lib_global_types::is_lib_global_type;
use crate::namespace::Resolution;
use crate::types::Type;

// A narrow overlay on top of the declared symbol types in CheckContext.symbols: a
// small association list, not a full map, since only a handful of variables are
// ever narrowed within one branch. Looked up before falling back to a symbol's
// declared type wherever narrowing might apply.
//
// Besides the per-symbol entries there are per-property-path entries (`user.address`,
// `a.b.c`), kept in their own list so the symbol API below is unchanged: a path entry is
// only ever read through get_path and never shadows a symbol's own entry.
#[derive(Default, Clone)]
pub struct NarrowState(Vec<(SymbolId, TypeId)>, Vec<PathEntry>);

#[derive(Clone)]
struct PathEntry {
    symbol_id: SymbolId,
    path: Vec<String>,
    type_id: TypeId,
}

impl NarrowState {
    pub fn new() -> Self {
        Self(Vec::new(), Vec::new())
    }

    fn from_single(symbol_id: SymbolId, type_id: TypeId) -> Self {
        Self(vec![(symbol_id, type_id)], Vec::new())
    }

    fn from_path(symbol_id: SymbolId, path: Vec<String>, type_id: TypeId) -> Self {
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

struct WriteCollector<'s> {
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
pub fn narrow_condition(
    test: &Expression,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> (NarrowState, NarrowState) {
    let (mut on_true, mut on_false) = narrow_condition_by_symbol(test, scoping, ctx);
    // Property paths (`user.address !== null`) are narrowed alongside, never instead:
    // a shape the symbol layer already understands yields nothing here.
    let (path_true, path_false) = narrow_condition_by_path(test, scoping, ctx);
    on_true.extend(path_true);
    on_false.extend(path_false);
    (on_true, on_false)
}

// The condition shapes that narrow a plain variable. Its own recursion (`!`, `&&`, `||`,
// parentheses) goes back through narrow_condition, so the path layer applies at every level.
fn narrow_condition_by_symbol(
    test: &Expression,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> (NarrowState, NarrowState) {
    match test {
        // oxc keeps parentheses as their own node instead of discarding them, so
        // `!(typeof x === "number")`'s inner expression is a ParenthesizedExpression
        // wrapping the BinaryExpression, not the BinaryExpression itself. Without
        // this arm that inner shape never matches anything below and the whole
        // condition narrows nothing, in either branch, silently.
        Expression::ParenthesizedExpression(inner) => {
            narrow_condition(&inner.expression, scoping, ctx)
        }

        Expression::BinaryExpression(bin) => {
            // `in` and `instanceof` are not equality at all, so they are handled before
            // the equality detection below, which would find nothing to match.
            match bin.operator {
                BinaryOperator::In => return narrow_by_in(bin, scoping, ctx),
                BinaryOperator::Instanceof => return narrow_by_instanceof(bin, scoping, ctx),
                _ => {}
            }

            // Treats == and != identically to === and !==, just flipping which
            // computed branch (true_type vs false_type) ends up where, so the
            // detection logic below only needs to handle the equality case once.
            let negated = matches!(
                bin.operator,
                BinaryOperator::Inequality | BinaryOperator::StrictInequality
            );
            let is_equality = matches!(
                bin.operator,
                BinaryOperator::Equality
                    | BinaryOperator::Inequality
                    | BinaryOperator::StrictEquality
                    | BinaryOperator::StrictInequality
            );
            if !is_equality {
                return empty_pair();
            }

            let is_loose = matches!(
                bin.operator,
                BinaryOperator::Equality | BinaryOperator::Inequality
            );

            if let Some((symbol_id, tag)) = typeof_check(&bin.left, &bin.right, scoping)
                .or_else(|| typeof_check(&bin.right, &bin.left, scoping))
            {
                // A tag no type in this checker can ever match, "bigint" or
                // "symbol", would filter every union member out and leave never
                // in the true branch, turning a gap in the type model into
                // errors on the source. Narrowing nothing is less precise but
                // never wrong.
                if !is_known_typeof_tag(&tag) {
                    return empty_pair();
                }
                return by_symbol(ctx, symbol_id, |arena, current, want_true| {
                    narrow_by_typeof(arena, current, &tag, want_true != negated)
                });
            }

            if let Some((symbol_id, is_null)) = nullish_check(&bin.left, &bin.right, scoping)
                .or_else(|| nullish_check(&bin.right, &bin.left, scoping))
            {
                return by_symbol(ctx, symbol_id, |arena, current, want_true| {
                    narrow_by_nullish(arena, current, is_null, is_loose, want_true != negated)
                });
            }

            // Loose equality against a literal (x == "a") is left alone: unlike
            // null/undefined, JavaScript's abstract-equality coercions for other
            // literals (0 == "0", "" == false) mean a matching member of x's own
            // union is not the only way this can pass, so narrowing to it could
            // remove a value the true branch can still actually hold.
            if !is_loose {
                if let Some((symbol_id, literal)) =
                    literal_check(&bin.left, &bin.right, scoping, &mut ctx.arena)
                        .or_else(|| literal_check(&bin.right, &bin.left, scoping, &mut ctx.arena))
                {
                    return by_symbol(ctx, symbol_id, move |arena, current, want_true| {
                        narrow_by_literal(arena, current, literal, want_true != negated)
                    });
                }
            }

            // `shape.kind === "circle"`: narrows `shape` itself, when it is a union of
            // object types, to the members whose `kind` can be that literal. Strict
            // equality only, for the same coercion reason as above.
            if !is_loose
                && let Some((symbol_id, property, literal)) =
                    member_literal_check(&bin.left, &bin.right, scoping, &mut ctx.arena).or_else(
                        || member_literal_check(&bin.right, &bin.left, scoping, &mut ctx.arena),
                    )
            {
                return by_symbol(ctx, symbol_id, move |arena, current, want_true| {
                    narrow_by_property_literals(
                        arena,
                        current,
                        &property,
                        &[literal],
                        want_true != negated,
                    )
                });
            }

            empty_pair()
        }

        // `a && b`: the true side needs both to hold; the false side is reached
        // either because `a` was false or because `a` held and `b` did not, so it
        // is the join of those two paths. `b` is narrowed against the state `a`
        // establishes, since it only runs when `a` held. `a || b` is the mirror
        // image. `??` narrows nothing here.
        Expression::LogicalExpression(logical) => match logical.operator {
            LogicalOperator::And => {
                let (left_true, left_false) = narrow_condition(&logical.left, scoping, ctx);
                let outer_narrow = ctx.narrow.clone();
                ctx.narrow.extend(left_true.clone());
                let (right_true, right_false) = narrow_condition(&logical.right, scoping, ctx);
                ctx.narrow = outer_narrow;

                let mut true_overlay = left_true.clone();
                true_overlay.extend(right_true);
                let mut right_false_path = left_true;
                right_false_path.extend(right_false);
                let false_overlay = join_states(&left_false, &right_false_path, &mut ctx.arena);
                (true_overlay, false_overlay)
            }
            LogicalOperator::Or => {
                let (left_true, left_false) = narrow_condition(&logical.left, scoping, ctx);
                let outer_narrow = ctx.narrow.clone();
                ctx.narrow.extend(left_false.clone());
                let (right_true, right_false) = narrow_condition(&logical.right, scoping, ctx);
                ctx.narrow = outer_narrow;

                let mut false_overlay = left_false.clone();
                false_overlay.extend(right_false);
                let mut right_true_path = left_false;
                right_true_path.extend(right_true);
                let true_overlay = join_states(&left_true, &right_true_path, &mut ctx.arena);
                (true_overlay, false_overlay)
            }
            LogicalOperator::Coalesce => empty_pair(),
        },

        // `!x`, `!(x === null)`, `!(typeof x === "string")`: whatever the inner
        // expression's own pair would be, just with the two branches swapped, so
        // this composes with every condition shape above and below for free.
        Expression::UnaryExpression(unary) if unary.operator == UnaryOperator::LogicalNot => {
            let (true_overrides, false_overrides) = narrow_condition(&unary.argument, scoping, ctx);
            (false_overrides, true_overrides)
        }

        Expression::Identifier(ident) => {
            let Some(symbol_id) = resolve_symbol_id(ident, scoping) else {
                return empty_pair();
            };
            by_symbol(ctx, symbol_id, narrow_truthy)
        }

        // `if (result.ok)`: the truthiness of a property narrows the object itself when it
        // is a union whose members disagree about that property, which is how a boolean
        // discriminant (`ok: true` against `ok: false`) is tested without a comparison.
        Expression::StaticMemberExpression(member) if !member.optional => {
            let Expression::Identifier(ident) = &member.object else {
                return empty_pair();
            };
            let Some(symbol_id) = resolve_symbol_id(ident, scoping) else {
                return empty_pair();
            };
            let property = member.property.name.to_string();
            by_symbol(ctx, symbol_id, move |arena, current, want_truthy| {
                narrow_by_property_truthiness(arena, current, &property, want_truthy)
            })
        }

        _ => empty_pair(),
    }
}

// A property chain rooted at a variable: `user.address.city` is the variable `user` with
// the path ["address", "city"]. Only plain `.name` accesses count; an optional chain,
// a computed access or a call in the middle of it has no stable identity to narrow.
pub(crate) struct PathRef {
    pub symbol_id: SymbolId,
    pub path: Vec<String>,
}

pub(crate) fn path_of_member(
    member: &StaticMemberExpression,
    scoping: &Scoping,
) -> Option<PathRef> {
    if member.optional {
        return None;
    }
    let mut path = vec![member.property.name.to_string()];
    let mut object = &member.object;
    loop {
        match object {
            Expression::StaticMemberExpression(inner) if !inner.optional => {
                path.push(inner.property.name.to_string());
                object = &inner.object;
            }
            Expression::ParenthesizedExpression(inner) => object = &inner.expression,
            Expression::Identifier(ident) => {
                let symbol_id = resolve_symbol_id(ident, scoping)?;
                path.reverse();
                return Some(PathRef { symbol_id, path });
            }
            _ => return None,
        }
    }
}

fn path_ref(expr: &Expression, scoping: &Scoping) -> Option<PathRef> {
    match expr {
        Expression::ParenthesizedExpression(inner) => path_ref(&inner.expression, scoping),
        Expression::StaticMemberExpression(member) => path_of_member(member, scoping),
        _ => None,
    }
}

// What `a.b` currently reads as because a condition or an assignment narrowed that exact
// path, if one did. None costs one length check when no path was ever narrowed.
pub(crate) fn narrowed_member_type(
    member: &StaticMemberExpression,
    scoping: &Scoping,
    ctx: &CheckContext<'_, '_>,
) -> Option<TypeId> {
    if !ctx.narrow.has_paths() {
        return None;
    }
    let target = path_of_member(member, scoping)?;
    ctx.narrow.get_path(target.symbol_id, &target.path)
}

// `typeof a.b === "string"`, `a.b === null`, `a.b !== undefined`, `a.b === "x"` and the
// truthiness of `a.b`, for a chain `path_ref` accepts. These are the shapes the symbol
// layer handles for a bare variable; the narrowing functions are the same ones.
fn narrow_condition_by_path(
    test: &Expression,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> (NarrowState, NarrowState) {
    match test {
        Expression::BinaryExpression(bin) => {
            let is_equality = matches!(
                bin.operator,
                BinaryOperator::Equality
                    | BinaryOperator::Inequality
                    | BinaryOperator::StrictEquality
                    | BinaryOperator::StrictInequality
            );
            if !is_equality {
                return empty_pair();
            }
            let negated = matches!(
                bin.operator,
                BinaryOperator::Inequality | BinaryOperator::StrictInequality
            );
            let is_loose = matches!(
                bin.operator,
                BinaryOperator::Equality | BinaryOperator::Inequality
            );

            for (first, second) in [(&bin.left, &bin.right), (&bin.right, &bin.left)] {
                if let Expression::UnaryExpression(unary) = first
                    && unary.operator == UnaryOperator::Typeof
                    && let Some(target) = path_ref(&unary.argument, scoping)
                    && let Expression::StringLiteral(tag) = second
                {
                    let tag = tag.value.to_string();
                    if !is_known_typeof_tag(&tag) {
                        return empty_pair();
                    }
                    return by_path(
                        ctx,
                        scoping,
                        &unary.argument,
                        target,
                        move |arena, current, want| {
                            narrow_by_typeof(arena, current, &tag, want != negated)
                        },
                    );
                }
            }

            for (first, second) in [(&bin.left, &bin.right), (&bin.right, &bin.left)] {
                let Some(target) = path_ref(first, scoping) else {
                    continue;
                };
                let is_null = match second {
                    Expression::NullLiteral(_) => true,
                    Expression::Identifier(other) if other.name == "undefined" => false,
                    _ => continue,
                };
                return by_path(ctx, scoping, first, target, move |arena, current, want| {
                    narrow_by_nullish(arena, current, is_null, is_loose, want != negated)
                });
            }

            // Strict equality only, for the same coercion reason the variable form gives.
            if !is_loose {
                for (first, second) in [(&bin.left, &bin.right), (&bin.right, &bin.left)] {
                    let Some(target) = path_ref(first, scoping) else {
                        continue;
                    };
                    let Some(literal) = resolve_literal(second, &mut ctx.arena) else {
                        continue;
                    };
                    return by_path(ctx, scoping, first, target, move |arena, current, want| {
                        narrow_by_literal(arena, current, literal, want != negated)
                    });
                }
            }
            empty_pair()
        }

        Expression::StaticMemberExpression(_) => match path_ref(test, scoping) {
            Some(target) => by_path(ctx, scoping, test, target, narrow_truthy),
            None => empty_pair(),
        },

        _ => empty_pair(),
    }
}

// The path counterpart of by_symbol. The path's current type comes from inferring the
// expression, which already reads an earlier narrowing of the same path, and whose own
// errors are dropped: the statement being checked reports them once, and this is a second
// look at the same expression.
fn by_path(
    ctx: &mut CheckContext<'_, '_>,
    scoping: &Scoping,
    expr: &Expression,
    target: PathRef,
    narrow: impl Fn(&mut TypeArena, TypeId, bool) -> TypeId,
) -> (NarrowState, NarrowState) {
    let mark = ctx.diagnostics.len();
    let current = crate::bridge::expressions::infer_expression_type(expr, scoping, ctx);
    ctx.diagnostics.truncate(mark);
    // A gap in this checker's model of the property would otherwise turn into errors on
    // the source, as the instanceof comment above describes.
    if current == ctx.arena.error() {
        return empty_pair();
    }
    let true_type = narrow(&mut ctx.arena, current, true);
    let false_type = narrow(&mut ctx.arena, current, false);
    (
        NarrowState::from_path(target.symbol_id, target.path.clone(), true_type),
        NarrowState::from_path(target.symbol_id, target.path, false_type),
    )
}

fn empty_pair() -> (NarrowState, NarrowState) {
    (NarrowState::new(), NarrowState::new())
}

fn by_symbol(
    ctx: &mut CheckContext<'_, '_>,
    symbol_id: SymbolId,
    narrow: impl Fn(&mut TypeArena, TypeId, bool) -> TypeId,
) -> (NarrowState, NarrowState) {
    // The overlay first, so narrowing composes: an inner condition refines what
    // the enclosing branch already established about this symbol rather than
    // starting again from its declared type and silently widening it back.
    let Some(current) = ctx
        .narrow
        .get(symbol_id)
        .or_else(|| ctx.symbols.get(symbol_id))
    else {
        return empty_pair();
    };
    let true_type = narrow(&mut ctx.arena, current, true);
    let false_type = narrow(&mut ctx.arena, current, false);
    (
        NarrowState::from_single(symbol_id, true_type),
        NarrowState::from_single(symbol_id, false_type),
    )
}

// The discriminant shapes a `switch` can narrow on: `kind`, `shape.kind` (the
// discriminated-union form) and `typeof x`. The narrowing overlay is keyed by a
// single symbol, so a discriminant that does not resolve to one (a call, an index,
// a longer chain) has nowhere to record a result and narrows nothing.
pub enum SwitchDiscriminant {
    Identifier(SymbolId),
    Property(SymbolId, String),
    Typeof(SymbolId),
}

pub fn switch_discriminant(expr: &Expression, scoping: &Scoping) -> Option<SwitchDiscriminant> {
    match expr {
        Expression::ParenthesizedExpression(inner) => {
            switch_discriminant(&inner.expression, scoping)
        }
        Expression::Identifier(ident) => {
            resolve_symbol_id(ident, scoping).map(SwitchDiscriminant::Identifier)
        }
        Expression::StaticMemberExpression(member) if !member.optional => {
            let Expression::Identifier(ident) = &member.object else {
                return None;
            };
            let symbol_id = resolve_symbol_id(ident, scoping)?;
            Some(SwitchDiscriminant::Property(
                symbol_id,
                member.property.name.to_string(),
            ))
        }
        Expression::UnaryExpression(unary) if unary.operator == UnaryOperator::Typeof => {
            let Expression::Identifier(ident) = &unary.argument else {
                return None;
            };
            resolve_symbol_id(ident, scoping).map(SwitchDiscriminant::Typeof)
        }
        _ => None,
    }
}

fn current_type(ctx: &CheckContext<'_, '_>, symbol_id: SymbolId) -> Option<TypeId> {
    ctx.narrow
        .get(symbol_id)
        .or_else(|| ctx.symbols.get(symbol_id))
}

// The narrowing for a case body reached through any of `tests` (several when empty
// case labels are grouped: `case "a": case "b": body`). If any one test cannot be
// resolved to a literal (or a known typeof tag) the body could be reached with a
// value this checker cannot describe, so nothing is narrowed.
pub fn narrow_switch_case(
    discriminant: &SwitchDiscriminant,
    tests: &[&Expression],
    ctx: &mut CheckContext<'_, '_>,
) -> NarrowState {
    match discriminant {
        SwitchDiscriminant::Identifier(symbol_id) => {
            let Some(literals) = resolve_all_literals(tests, &mut ctx.arena) else {
                return NarrowState::new();
            };
            let Some(current) = current_type(ctx, *symbol_id) else {
                return NarrowState::new();
            };
            let narrowed = narrow_by_literals(&mut ctx.arena, current, &literals, true);
            NarrowState::from_single(*symbol_id, narrowed)
        }
        SwitchDiscriminant::Property(symbol_id, property) => {
            let Some(literals) = resolve_all_literals(tests, &mut ctx.arena) else {
                return NarrowState::new();
            };
            let Some(current) = current_type(ctx, *symbol_id) else {
                return NarrowState::new();
            };
            let narrowed =
                narrow_by_property_literals(&mut ctx.arena, current, property, &literals, true);
            NarrowState::from_single(*symbol_id, narrowed)
        }
        SwitchDiscriminant::Typeof(symbol_id) => {
            let Some(tags) = resolve_all_typeof_tags(tests) else {
                return NarrowState::new();
            };
            let Some(current) = current_type(ctx, *symbol_id) else {
                return NarrowState::new();
            };
            let mut parts = Vec::with_capacity(tags.len());
            for tag in &tags {
                parts.push(narrow_by_typeof(&mut ctx.arena, current, tag, true));
            }
            let narrowed = ctx.arena.alloc_union(parts);
            NarrowState::from_single(*symbol_id, narrowed)
        }
    }
}

// A switch's default: whatever remains once every sibling case's test is ruled
// out. A test this checker cannot resolve is simply left out of the exclusion,
// which keeps more members than strictly necessary and is always safe.
pub fn narrow_switch_default(
    discriminant: &SwitchDiscriminant,
    tests: &[&Expression],
    ctx: &mut CheckContext<'_, '_>,
) -> NarrowState {
    match discriminant {
        SwitchDiscriminant::Identifier(symbol_id) => {
            let literals: Vec<TypeId> = tests
                .iter()
                .filter_map(|test| resolve_literal(test, &mut ctx.arena))
                .collect();
            let Some(current) = current_type(ctx, *symbol_id) else {
                return NarrowState::new();
            };
            let narrowed = narrow_by_literals(&mut ctx.arena, current, &literals, false);
            NarrowState::from_single(*symbol_id, narrowed)
        }
        SwitchDiscriminant::Property(symbol_id, property) => {
            let literals: Vec<TypeId> = tests
                .iter()
                .filter_map(|test| resolve_literal(test, &mut ctx.arena))
                .collect();
            let Some(current) = current_type(ctx, *symbol_id) else {
                return NarrowState::new();
            };
            let narrowed =
                narrow_by_property_literals(&mut ctx.arena, current, property, &literals, false);
            NarrowState::from_single(*symbol_id, narrowed)
        }
        SwitchDiscriminant::Typeof(symbol_id) => {
            let Some(mut narrowed) = current_type(ctx, *symbol_id) else {
                return NarrowState::new();
            };
            for test in tests {
                if let Expression::StringLiteral(tag) = test
                    && is_known_typeof_tag(&tag.value)
                {
                    narrowed = narrow_by_typeof(&mut ctx.arena, narrowed, &tag.value, false);
                }
            }
            NarrowState::from_single(*symbol_id, narrowed)
        }
    }
}

fn resolve_all_literals(tests: &[&Expression], arena: &mut TypeArena) -> Option<Vec<TypeId>> {
    let mut literals = Vec::with_capacity(tests.len());
    for test in tests {
        literals.push(resolve_literal(test, arena)?);
    }
    Some(literals)
}

// One tag that is not a known `typeof` result gives up on the whole switch, for the reason
// typeof_check gives: a tag no type here can match would rule out every member and leave
// `never`.
fn resolve_all_typeof_tags(tests: &[&Expression]) -> Option<Vec<String>> {
    let mut tags = Vec::with_capacity(tests.len());
    for test in tests {
        let Expression::StringLiteral(tag) = test else {
            return None;
        };
        if !is_known_typeof_tag(&tag.value) {
            return None;
        }
        tags.push(tag.value.to_string());
    }
    Some(tags)
}

// The literal-resolution half of literal_check, split out so the switch narrowing
// above can reuse it without needing a second identifier to match against; a
// case's test is compared to a discriminant already known by symbol_id, not
// discovered from an ident/other pair the way a binary expression's two operands
// are.
fn resolve_literal(expr: &Expression, arena: &mut TypeArena) -> Option<TypeId> {
    match expr {
        Expression::StringLiteral(lit) => {
            Some(arena.alloc(Type::StringLiteral(lit.value.to_string())))
        }
        Expression::NumericLiteral(lit) => Some(arena.alloc(Type::NumberLiteral(lit.value))),
        Expression::BooleanLiteral(lit) => Some(arena.alloc(Type::BooleanLiteral(lit.value))),
        _ => None,
    }
}

pub fn resolve_symbol_id(ident: &IdentifierReference, scoping: &Scoping) -> Option<SymbolId> {
    ident
        .reference_id
        .get()
        .and_then(|reference_id| scoping.get_reference(reference_id).symbol_id())
}

fn typeof_check(
    maybe_typeof: &Expression,
    maybe_tag: &Expression,
    scoping: &Scoping,
) -> Option<(SymbolId, String)> {
    let Expression::UnaryExpression(unary) = maybe_typeof else {
        return None;
    };
    if unary.operator != UnaryOperator::Typeof {
        return None;
    }
    let Expression::Identifier(ident) = &unary.argument else {
        return None;
    };
    let symbol_id = resolve_symbol_id(ident, scoping)?;
    let Expression::StringLiteral(tag) = maybe_tag else {
        return None;
    };
    Some((symbol_id, tag.value.to_string()))
}

// The variable a comparison is really about. `(line = read()) !== null` compares the
// value just assigned to `line`, so the check narrows `line`; parentheses around a plain
// name change nothing. Only a plain `=` to a plain name counts: `+=` stores something
// other than the right-hand side, and a member target is a property path's business.
fn narrowed_identifier<'a, 'b>(expr: &'a Expression<'b>) -> Option<&'a IdentifierReference<'b>> {
    match expr {
        Expression::Identifier(ident) => Some(ident),
        Expression::ParenthesizedExpression(inner) => narrowed_identifier(&inner.expression),
        Expression::AssignmentExpression(assign)
            if assign.operator == oxc_ast::ast::AssignmentOperator::Assign =>
        {
            match &assign.left {
                AssignmentTarget::AssignmentTargetIdentifier(ident) => Some(ident),
                _ => None,
            }
        }
        _ => None,
    }
}

fn nullish_check(
    maybe_ident: &Expression,
    maybe_nullish: &Expression,
    scoping: &Scoping,
) -> Option<(SymbolId, bool)> {
    let ident = narrowed_identifier(maybe_ident)?;
    let symbol_id = resolve_symbol_id(ident, scoping)?;
    match maybe_nullish {
        Expression::NullLiteral(_) => Some((symbol_id, true)),

        Expression::Identifier(other) if other.name == "undefined" => Some((symbol_id, false)),
        _ => None,
    }
}

// Recognizes `ident === <literal>`, where the literal is a value this checker's
// type model already represents as its own TypeId (a string, number, or boolean
// literal), matching how a discriminant property or a narrowed union is typically
// spelled: "circle", 0, true. Anything else on the literal side (an identifier, a
// computed expression) is not something narrow_by_literal has a TypeId to compare
// union members against, so it returns None and the caller falls through to
// empty_pair the same way an unrecognized shape always has. Takes the arena
// directly, unlike nullish_check and typeof_check, since allocating the literal's
// TypeId (interned, so a repeated "circle" always resolves to the same id; see
// narrow_by_literal) needs a mutable arena, not just the shared Scoping the other
// *_check helpers read from.
fn literal_check(
    maybe_ident: &Expression,
    maybe_literal: &Expression,
    scoping: &Scoping,
    arena: &mut TypeArena,
) -> Option<(SymbolId, TypeId)> {
    let Expression::Identifier(ident) = maybe_ident else {
        return None;
    };
    let symbol_id = resolve_symbol_id(ident, scoping)?;
    let literal = resolve_literal(maybe_literal, arena)?;
    Some((symbol_id, literal))
}

// `"radius" in shape`: narrows a union of object types by whether a member has the
// property. Only a literal key and a plain identifier are recognised.
fn narrow_by_in(
    bin: &BinaryExpression,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> (NarrowState, NarrowState) {
    let Expression::StringLiteral(key) = &bin.left else {
        return empty_pair();
    };
    let Expression::Identifier(ident) = &bin.right else {
        return empty_pair();
    };
    let Some(symbol_id) = resolve_symbol_id(ident, scoping) else {
        return empty_pair();
    };
    let property = key.value.to_string();
    by_symbol(ctx, symbol_id, move |arena, current, want_present| {
        narrow_by_property_presence(arena, current, &property, want_present)
    })
}

// How a union member relates to a property name, as far as `in` can tell.
enum Presence {
    Required,
    Optional,
    Absent,
    // Not an object, or a Record whose keys are not modelled: it may or may not have
    // the property, so `in` can neither keep nor drop it with confidence.
    Unknown,
}

// Absent is only said of an object that lists its properties. A non-object, and a Record
// (its keys come from K, not from a property list), read as Unknown, so neither branch
// drops them.
fn property_presence(arena: &TypeArena, member: TypeId, property: &str) -> Presence {
    let Type::Object(object) = arena.get(member) else {
        return Presence::Unknown;
    };
    if arena.record_value_type(member).is_some() {
        return Presence::Unknown;
    }
    match object
        .properties
        .iter()
        .find(|entry| &*entry.name == property)
    {
        Some(entry) if entry.optional => Presence::Optional,
        Some(_) => Presence::Required,
        None => Presence::Absent,
    }
}

// The true branch keeps the members that have, or may have, the property; the false
// branch drops only the members where it is required, since an optional or missing
// property can still be absent. A lone object type is returned unchanged: tsc would
// intersect it with a record of the key, which this checker has no way to express.
// If no member could have the property the union is also left alone, for the same
// reason, rather than collapsing to never.
fn narrow_by_property_presence(
    arena: &mut TypeArena,
    id: TypeId,
    property: &str,
    want_present: bool,
) -> TypeId {
    let Some(members) = union_members(arena, id) else {
        return id;
    };
    let presences: Vec<Presence> = members
        .iter()
        .map(|&member| property_presence(arena, member, property))
        .collect();

    if want_present && presences.iter().all(|p| matches!(p, Presence::Absent)) {
        return id;
    }

    let mut kept = Vec::with_capacity(members.len());
    for (member, presence) in members.iter().zip(&presences) {
        let keep = if want_present {
            !matches!(presence, Presence::Absent)
        } else {
            !matches!(presence, Presence::Required)
        };
        if keep {
            kept.push(*member);
        }
    }
    arena.alloc_union(kept)
}

// `pet instanceof Dog`: keeps the members that are instances of the class, using the
// checker's own assignability, so a subclass counts as its parent. A member that is a
// supertype of the class (declared `Animal`, tested against `Dog`) becomes the class in
// the true branch. Types are structural here, as in TypeScript, so two classes with
// identical shapes cannot be told apart by instanceof, in tsc either.
//
// `any` and `unknown` become the class in the true branch and stay as they are in the
// false one. The error type and a generic parameter are left alone on both sides,
// because narrowing them would turn a gap in this checker's model into errors on valid
// code. The right-hand side must resolve to a declared type; anything else (a
// variable, a call, an unknown name) narrows nothing.
fn narrow_by_instanceof(
    bin: &BinaryExpression,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> (NarrowState, NarrowState) {
    let Expression::Identifier(value) = &bin.left else {
        return empty_pair();
    };
    let Expression::Identifier(class) = &bin.right else {
        return empty_pair();
    };
    let Some(symbol_id) = resolve_symbol_id(value, scoping) else {
        return empty_pair();
    };
    let resolution = ctx.namespace.resolve(&class.name, &mut ctx.arena);
    let Resolution::Resolved(target) = resolution else {
        // A name the file binds itself (a variable holding a constructor) says nothing
        // about the type; one it does not bind may be a standard library class.
        if resolve_symbol_id(class, scoping).is_some() {
            return empty_pair();
        }
        return narrow_by_lib_instanceof(&class.name, symbol_id, ctx);
    };
    let Some(current) = current_type(ctx, symbol_id) else {
        return empty_pair();
    };

    let members = union_members(&ctx.arena, current).unwrap_or_else(|| vec![current]);
    let mut true_members = Vec::with_capacity(members.len());
    let mut false_members = Vec::with_capacity(members.len());
    for member in members {
        let (open_ended, unknowable) = {
            let ty = ctx.arena.get(member);
            (
                matches!(ty, Type::Any | Type::Unknown),
                matches!(ty, Type::Error | Type::GenericParameter(..)),
            )
        };
        if unknowable {
            true_members.push(member);
            false_members.push(member);
        } else if open_ended {
            true_members.push(target);
            false_members.push(member);
        } else if ctx.semantic().is_assignable(member, target) {
            true_members.push(member);
        } else {
            false_members.push(member);
            if ctx.semantic().is_assignable(target, member) {
                true_members.push(target);
            }
        }
    }
    let true_type = ctx.arena.alloc_union(true_members);
    let false_type = ctx.arena.alloc_union(false_members);
    (
        NarrowState::from_single(symbol_id, true_type),
        NarrowState::from_single(symbol_id, false_type),
    )
}

// How `instanceof` reads a standard library class the file does not declare. `Promise`
// and `Array` have types here, so their instances can be told apart; every other class
// (`Date`, `Error`, `Map`, ...) has no type and reads as `any`, like a `new Date()`.
#[derive(Clone, Copy)]
enum LibClass {
    Promise,
    Array,
    Unmodelled,
}

fn lib_class(name: &str) -> Option<LibClass> {
    match name {
        "Promise" => Some(LibClass::Promise),
        "Array" => Some(LibClass::Array),
        _ if is_lib_global_type(name) => Some(LibClass::Unmodelled),
        _ => None,
    }
}

// `value instanceof Date` where `Date` comes from the standard library. `unknown` and
// `any` become the class in the true branch, which for a class without a type is `any`,
// so `val.getTime()` is not an error, and stay as they are in the false one.
//
// For `Promise` and `Array` a member is kept in the true branch if it is one and in the
// false branch if it is not, so `fn instanceof Promise` leaves the function member of
// `F | Promise<T>` behind. A true branch with nothing left keeps the type it had, as a
// member this checker cannot place is not worth a `never`. Any other class narrows only
// `any` and `unknown`; a member it cannot tell apart stays on both sides.
fn narrow_by_lib_instanceof(
    class: &str,
    symbol_id: SymbolId,
    ctx: &mut CheckContext<'_, '_>,
) -> (NarrowState, NarrowState) {
    let Some(kind) = lib_class(class) else {
        return empty_pair();
    };
    let Some(current) = current_type(ctx, symbol_id) else {
        return empty_pair();
    };

    let any = ctx.arena.any();
    let target = match kind {
        LibClass::Promise => ctx.arena.alloc_promise(any),
        LibClass::Array => ctx.arena.alloc(Type::Array(any)),
        LibClass::Unmodelled => any,
    };
    let members = union_members(&ctx.arena, current).unwrap_or_else(|| vec![current]);
    let mut true_members = Vec::with_capacity(members.len());
    let mut false_members = Vec::with_capacity(members.len());
    for member in members {
        let ty = ctx.arena.get(member);
        let open_ended = matches!(ty, Type::Any | Type::Unknown);
        let unknowable = matches!(ty, Type::Error | Type::GenericParameter(..));
        let instance = match kind {
            LibClass::Promise => ctx.arena.is_promise(member),
            LibClass::Array => matches!(ty, Type::Array(_)),
            LibClass::Unmodelled => false,
        };
        if open_ended {
            true_members.push(target);
            false_members.push(member);
        } else if unknowable || matches!(kind, LibClass::Unmodelled) {
            true_members.push(member);
            false_members.push(member);
        } else if instance {
            true_members.push(member);
        } else {
            false_members.push(member);
        }
    }
    let true_type = if true_members.is_empty() {
        current
    } else {
        ctx.arena.alloc_union(true_members)
    };
    let false_type = ctx.arena.alloc_union(false_members);
    (
        NarrowState::from_single(symbol_id, true_type),
        NarrowState::from_single(symbol_id, false_type),
    )
}

// `ident.property === <literal>` (either order) is how a discriminated union is
// narrowed. An optional chain is left out because `ident?.property` can also be
// undefined, which this narrowing does not model.
fn member_literal_check(
    maybe_member: &Expression,
    maybe_literal: &Expression,
    scoping: &Scoping,
    arena: &mut TypeArena,
) -> Option<(SymbolId, String, TypeId)> {
    let Expression::StaticMemberExpression(member) = maybe_member else {
        return None;
    };
    if member.optional {
        return None;
    }
    let Expression::Identifier(ident) = &member.object else {
        return None;
    };
    let symbol_id = resolve_symbol_id(ident, scoping)?;
    let literal = resolve_literal(maybe_literal, arena)?;
    Some((symbol_id, member.property.name.to_string(), literal))
}

fn is_known_typeof_tag(tag: &str) -> bool {
    matches!(
        tag,
        "string" | "number" | "boolean" | "undefined" | "object" | "function"
    )
}

// `typeof null` is "object" in JavaScript, so null belongs with objects and
// arrays here rather than with the primitives, and a `typeof x === "object"`
// check therefore does not remove null from x.
fn matches_typeof_tag(arena: &TypeArena, id: TypeId, tag: &str) -> bool {
    matches!(
        (arena.get(id), tag),
        (Type::String | Type::StringLiteral(_), "string")
            | (Type::Number | Type::NumberLiteral(_), "number")
            | (Type::Boolean | Type::BooleanLiteral(_), "boolean")
            | (Type::Undefined, "undefined")
            | (
                Type::Object(_) | Type::Array(_) | Type::Tuple(_) | Type::Null,
                "object"
            )
            | (Type::Function(_), "function")
    )
}

// `unknown` becomes the matching primitive in the true branch (`typeof x ===
// "string"` makes an unknown `x` a string) and stays unknown otherwise. `any`, the
// error type and a generic parameter are kept as they are on both sides: narrowing
// them to never would turn a gap in this checker's model into errors on valid
// source. A lone (non-union) type is handled as a one-member union, so one that
// does not match still ends up as never.
fn narrow_by_typeof(arena: &mut TypeArena, id: TypeId, tag: &str, want_match: bool) -> TypeId {
    let members = union_members(arena, id).unwrap_or_else(|| vec![id]);
    let mut kept = Vec::with_capacity(members.len());
    for member in members {
        let opaque = matches!(
            arena.get(member),
            Type::Any | Type::Error | Type::GenericParameter(..)
        );
        let is_unknown = matches!(arena.get(member), Type::Unknown);
        if opaque {
            kept.push(member);
        } else if is_unknown {
            if want_match {
                kept.push(primitive_for_tag(arena, tag).unwrap_or(member));
            } else {
                kept.push(member);
            }
        } else if matches_typeof_tag(arena, member, tag) == want_match {
            kept.push(member);
        }
    }
    arena.alloc_union(kept)
}

fn primitive_for_tag(arena: &TypeArena, tag: &str) -> Option<TypeId> {
    match tag {
        "string" => Some(arena.string()),
        "number" => Some(arena.number()),
        "boolean" => Some(arena.boolean()),
        "undefined" => Some(arena.undefined()),
        _ => None,
    }
}

fn narrow_by_literal(
    arena: &mut TypeArena,
    id: TypeId,
    literal: TypeId,
    want_match: bool,
) -> TypeId {
    narrow_by_literals(arena, id, &[literal], want_match)
}

// Narrows by equality against a set of literals.
// Matching direction: a wide member (`string` for "a", `number` for 1, `boolean`
// for true, `unknown` for anything) is replaced by the literals it covers, as tsc
// does. Keeping the wide member would lose the narrowing, and dropping it would
// turn `if (s === "a")` on a plain `string` into `never` and flag valid code.
// `any`, the error type and a generic parameter are kept as they are, because a
// guessed narrowing for them would turn a gap in this checker into false errors.
// Excluding direction: only an exact literal member is removed, since `string`
// minus "a" is still `string`. `boolean` is the exception because it is exactly
// true | false, so removing one leaves the other.
// Literals are compared by TypeId, which is sound only because every literal Type
// is interned.
fn narrow_by_literals(
    arena: &mut TypeArena,
    id: TypeId,
    literals: &[TypeId],
    want_match: bool,
) -> TypeId {
    let members = union_members(arena, id).unwrap_or_else(|| vec![id]);
    let mut kept: Vec<TypeId> = Vec::with_capacity(members.len());
    for member in members {
        if want_match {
            keep_matching(arena, member, literals, &mut kept);
        } else {
            keep_remaining(arena, member, literals, &mut kept);
        }
    }
    arena.alloc_union(kept)
}

fn literal_widens_to(arena: &TypeArena, wide: TypeId, literal: TypeId) -> bool {
    matches!(
        (arena.get(wide), arena.get(literal)),
        (Type::String, Type::StringLiteral(_))
            | (Type::Number, Type::NumberLiteral(_))
            | (Type::Boolean, Type::BooleanLiteral(_))
    )
}

// The members of a union that can still equal one of the literals. A member that is one of
// them stays. `any`, the error type and a type parameter stay because nothing is known
// against them, `unknown` becomes the literals themselves, and a primitive keeps the
// literals it widens from (`string` with "a" leaves "a").
fn keep_matching(arena: &TypeArena, member: TypeId, literals: &[TypeId], kept: &mut Vec<TypeId>) {
    if literals.contains(&member) {
        kept.push(member);
        return;
    }
    match arena.get(member) {
        Type::Any | Type::Error | Type::GenericParameter(..) => kept.push(member),
        Type::Unknown => kept.extend_from_slice(literals),
        Type::String | Type::Number | Type::Boolean => kept.extend(
            literals
                .iter()
                .copied()
                .filter(|&literal| literal_widens_to(arena, member, literal)),
        ),
        _ => {}
    }
}

// The false side of the same test: the members left once the literals are ruled out.
// `boolean` is `true | false` under one id, so ruling out one literal has to leave the
// other, which is what `x !== true` must give.
fn keep_remaining(
    arena: &mut TypeArena,
    member: TypeId,
    literals: &[TypeId],
    kept: &mut Vec<TypeId>,
) {
    if literals.contains(&member) {
        return;
    }
    let excludes_a_boolean = literals
        .iter()
        .any(|&literal| matches!(arena.get(literal), Type::BooleanLiteral(_)));
    if matches!(arena.get(member), Type::Boolean) && excludes_a_boolean {
        for value in [true, false] {
            let excluded = literals.iter().any(
                |&literal| matches!(arena.get(literal), Type::BooleanLiteral(b) if *b == value),
            );
            if !excluded {
                kept.push(arena.alloc(Type::BooleanLiteral(value)));
            }
        }
        return;
    }
    kept.push(member);
}

// Narrows a union of object types by one property's literal type (the
// discriminated-union case). A lone object type is returned unchanged, since there
// is nothing to discriminate between. A member that is not an object, or lacks the
// property, is kept: this cannot prove it does not match, and dropping a member
// that does match would report valid code as an error. Matching keeps a member
// whose property type can be one of the literals; excluding removes a member only
// when its property type is exactly one literal, because a property typed
// "a" | "b" could still be "b".
fn narrow_by_property_literals(
    arena: &mut TypeArena,
    id: TypeId,
    property: &str,
    literals: &[TypeId],
    want_match: bool,
) -> TypeId {
    let Some(members) = union_members(arena, id) else {
        return id;
    };
    let mut kept = Vec::with_capacity(members.len());
    for member in members {
        let property_type = match arena.get(member) {
            Type::Object(object) => object
                .properties
                .iter()
                .find(|entry| &*entry.name == property)
                .map(|entry| entry.type_id),
            _ => None,
        };
        let keep = match property_type {
            None => true,
            Some(property_type) => {
                let candidates =
                    union_members(arena, property_type).unwrap_or_else(|| vec![property_type]);
                if want_match {
                    candidates.iter().any(|&candidate| {
                        literals
                            .iter()
                            .any(|&literal| property_may_equal(arena, candidate, literal))
                    })
                } else {
                    !(candidates.len() == 1 && literals.contains(&candidates[0]))
                }
            }
        };
        if keep {
            kept.push(member);
        }
    }
    arena.alloc_union(kept)
}

fn property_may_equal(arena: &TypeArena, candidate: TypeId, literal: TypeId) -> bool {
    if candidate == literal {
        return true;
    }
    // matches! rather than a match with `=> true` arms: every arm is a bare bool,
    // which clippy::match_like_matches_macro rejects under -D warnings.
    matches!(
        (arena.get(candidate), arena.get(literal)),
        (
            Type::Any | Type::Unknown | Type::Error | Type::GenericParameter(..),
            _
        ) | (Type::String, Type::StringLiteral(_))
            | (Type::Number, Type::NumberLiteral(_))
            | (Type::Boolean, Type::BooleanLiteral(_))
    )
}

fn narrow_by_nullish(
    arena: &mut TypeArena,
    id: TypeId,
    target_is_null: bool,
    is_loose: bool,
    want_match: bool,
) -> TypeId {
    let is_target = |arena: &TypeArena, t: TypeId| {
        // `==`/`!=` against either `null` or `undefined` matches both: this is
        // JavaScript's specific loose-equality-with-null quirk, not general
        // loose equality, and it is symmetric: `x == undefined` matches a
        // `null` value too, the same as `x == null` does. `===`/`!==` only ever
        // matches the exact literal on the right, so target_is_null still
        // decides which single type counts there.
        if is_loose {
            matches!(arena.get(t), Type::Null | Type::Undefined)
        } else if target_is_null {
            matches!(arena.get(t), Type::Null)
        } else {
            matches!(arena.get(t), Type::Undefined)
        }
    };
    // Problem: `any` and `unknown` are not null and not undefined, so both branches
    // of `x === null` filtered them out and `x` became never; a read of `x.foo` in
    // the true branch then reported a property missing on never.
    // Picked: a type that can hold anything (any, unknown), the error sentinel and a
    // type parameter are kept in both branches, because the test cannot rule them
    // in or out.
    // Cost: the true branch keeps `unknown` as unknown where tsc would say null.
    let is_open = |arena: &TypeArena, t: TypeId| {
        matches!(
            arena.get(t),
            Type::Any | Type::Unknown | Type::Error | Type::GenericParameter(..)
        )
    };
    match union_members(arena, id) {
        Some(members) => {
            let filtered = members
                .into_iter()
                .filter(|&m| is_open(arena, m) || is_target(arena, m) == want_match)
                .collect();
            arena.alloc_union(filtered)
        }
        None => {
            if is_open(arena, id) || is_target(arena, id) == want_match {
                id
            } else {
                arena.never()
            }
        }
    }
}

// Conservative on purpose: only returns true for a type that is provably falsy in
// every case, such as the literal 0 or an empty string literal. A plain, un-narrowed
// number or string is neither definitely falsy nor definitely truthy, so it is left
// alone by narrow_truthy below rather than guessed at.
fn is_definitely_falsy(arena: &TypeArena, id: TypeId) -> bool {
    match arena.get(id) {
        Type::Null | Type::Undefined | Type::BooleanLiteral(false) => true,
        Type::NumberLiteral(n) => *n == 0.0,
        Type::StringLiteral(s) => s.is_empty(),
        _ => false,
    }
}

fn is_definitely_truthy(arena: &TypeArena, id: TypeId) -> bool {
    match arena.get(id) {
        Type::Object(_)
        | Type::Array(_)
        | Type::Tuple(_)
        | Type::Function(_)
        | Type::BooleanLiteral(true) => true,
        Type::NumberLiteral(n) => *n != 0.0,
        Type::StringLiteral(s) => !s.is_empty(),
        _ => false,
    }
}

// Keeps what can still be truthy (or falsy). Only a member that is certainly the other
// kind is dropped, so a doubt keeps the member: a narrowing may lose precision and must
// never invent it.
fn narrow_truthy(arena: &mut TypeArena, id: TypeId, want_truthy: bool) -> TypeId {
    match union_members(arena, id) {
        Some(members) => {
            let filtered = members
                .into_iter()
                .filter(|&m| {
                    if want_truthy {
                        !is_definitely_falsy(arena, m)
                    } else {
                        !is_definitely_truthy(arena, m)
                    }
                })
                .collect();
            arena.alloc_union(filtered)
        }
        None => {
            let keep = if want_truthy {
                !is_definitely_falsy(arena, id)
            } else {
                !is_definitely_truthy(arena, id)
            };
            if keep { id } else { arena.never() }
        }
    }
}

// Narrows a union of object types by whether one of their properties is truthy. Only a
// union narrows; a member that is not an object, or has no such property, is kept. A
// member stays on the truthy side unless its property is always falsy, and on the falsy
// side unless its property is always truthy, using the same conservative notions as
// narrow_truthy: a plain `number` or `string` can be either, so it is never dropped.
fn narrow_by_property_truthiness(
    arena: &mut TypeArena,
    id: TypeId,
    property: &str,
    want_truthy: bool,
) -> TypeId {
    let Some(members) = union_members(arena, id) else {
        return id;
    };
    let mut kept = Vec::with_capacity(members.len());
    for member in members {
        let property_type = match arena.get(member) {
            Type::Object(object) => object
                .properties
                .iter()
                .find(|entry| &*entry.name == property)
                .map(|entry| entry.type_id),
            _ => None,
        };
        let keep = match property_type {
            None => true,
            Some(property_type) => {
                let candidates =
                    union_members(arena, property_type).unwrap_or_else(|| vec![property_type]);
                candidates.iter().any(|&candidate| {
                    if want_truthy {
                        !is_definitely_falsy(arena, candidate)
                    } else {
                        !is_definitely_truthy(arena, candidate)
                    }
                })
            }
        };
        if keep {
            kept.push(member);
        }
    }
    arena.alloc_union(kept)
}

fn union_members(arena: &TypeArena, id: TypeId) -> Option<Vec<TypeId>> {
    match arena.get(id) {
        Type::Union(members) => Some(members.clone()),
        _ => None,
    }
}

pub fn narrow_to_truthy(arena: &mut TypeArena, id: TypeId) -> TypeId {
    narrow_truthy(arena, id, true)
}

pub fn narrow_to_falsy(arena: &mut TypeArena, id: TypeId) -> TypeId {
    narrow_truthy(arena, id, false)
}

// Takes `null` and `undefined` out of a type: the value that is left once the nullish case
// is ruled out. It is the left side of `??`, the result of `!`, a bound name that has a
// default, and the operand of a read or an operation after the nullish error was already
// reported. A lone `null` or `undefined` leaves `never`, which the caller reads as nothing
// left to use.
pub fn narrow_to_non_nullish(arena: &mut TypeArena, id: TypeId) -> TypeId {
    let is_nullish =
        |arena: &TypeArena, t: TypeId| matches!(arena.get(t), Type::Null | Type::Undefined);
    match union_members(arena, id) {
        Some(members) => {
            let filtered = members
                .into_iter()
                .filter(|&m| !is_nullish(arena, m))
                .collect();
            arena.alloc_union(filtered)
        }
        None => {
            if is_nullish(arena, id) {
                arena.never()
            } else {
                id
            }
        }
    }
}
