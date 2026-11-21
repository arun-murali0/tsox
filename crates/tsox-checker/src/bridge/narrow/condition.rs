// Owns: the entry points `narrow_condition` and `narrow_condition_by_path`, the combinators
// (`&&`, `||`, `!`, parentheses, `??`) and optional-chain roots. Equality forms live in
// `equality.rs`; `in` in `in_operator.rs`; `instanceof` in `instanceof.rs`.

use super::*;

// `const isString = typeof x === "string"`: remembers what the condition narrows on each edge. Only
// variables nothing assigns again take part (a `const`, a parameter never assigned), as in tsc.
pub fn register_aliased_condition(
    alias: SymbolId,
    init: &Expression,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) {
    let (mut on_true, mut on_false) = narrow_condition(init, scoping, ctx);
    let stable = |symbol_id: SymbolId| {
        if symbol_id == THIS_ROOT {
            return true;
        }
        let flags = scoping.symbol_flags(symbol_id);
        let reassignable = flags.is_block_scoped() && !flags.is_const_variable();
        !reassignable && !scoping.symbol_is_mutated(symbol_id)
    };
    on_true.retain_symbols(stable);
    on_false.retain_symbols(stable);
    if !on_true.is_empty() || !on_false.is_empty() {
        ctx.aliased_conditions.insert(alias, (on_true, on_false));
    }
}

pub fn narrow_condition(
    test: &Expression,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> (NarrowState, NarrowState) {
    let (on_true, on_false) = narrow_condition_inner(test, scoping, ctx);
    let on_true = propagate_destructured(ctx, on_true);
    let on_false = propagate_destructured(ctx, on_false);
    (on_true, on_false)
}

pub(super) fn narrow_condition_inner(
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

// The condition shapes that narrow a plain variable, as a first-match table (`NARROWERS`, in
// `narrowers.rs`). Its own recursion (`!`, `&&`, `||`, parentheses) goes back through
// narrow_condition, so the path layer applies at every level.
pub(super) fn narrow_condition_by_symbol(
    test: &Expression,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> (NarrowState, NarrowState) {
    super::narrowers::NARROWERS
        .iter()
        .find_map(|narrower| (narrower.run)(test, scoping, ctx))
        .unwrap_or_else(empty_pair)
}

// A property chain rooted at a variable: `user.address.city` is the variable `user` with
// the path ["address", "city"]. Only plain `.name` accesses count; an optional chain,
// a computed access or a call in the middle of it has no stable identity to narrow.
pub struct PathRef {
    pub symbol_id: SymbolId,
    pub path: Vec<String>,
}

/// The root of a narrowed path that starts at `this` (`this._x`). `this` is no symbol, so it
/// gets one the scope tree never hands out; a narrowing state is per function body, where
/// `this` does not change.
pub const THIS_ROOT: SymbolId = SymbolId::new((u32::MAX - 1) as usize);

pub fn path_of_member(member: &StaticMemberExpression, scoping: &Scoping) -> Option<PathRef> {
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
            Expression::ThisExpression(_) => {
                path.reverse();
                return Some(PathRef {
                    symbol_id: THIS_ROOT,
                    path,
                });
            }
            _ => return None,
        }
    }
}

pub(super) fn path_ref(expr: &Expression, scoping: &Scoping) -> Option<PathRef> {
    match expr {
        Expression::ParenthesizedExpression(inner) => path_ref(&inner.expression, scoping),
        Expression::StaticMemberExpression(member) => path_of_member(member, scoping),
        _ => None,
    }
}

// What `a.b` currently reads as because a condition or an assignment narrowed that exact
// path, if one did. None costs one length check when no path was ever narrowed.
pub fn narrowed_member_type(
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
pub(super) fn narrow_condition_by_path(
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
pub(super) fn by_path(
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

pub(super) fn empty_pair() -> (NarrowState, NarrowState) {
    (NarrowState::new(), NarrowState::new())
}

// The variable an optional chain starts from, when the first link after it is the `?.`:
// `o?.a.b`, `o?.[k]`, `f?.(x)`. A `?.` further along says nothing about the root.
pub(super) fn chain_root(expr: &Expression, scoping: &Scoping) -> Option<SymbolId> {
    use oxc_ast::ast::ChainElement;
    let Expression::ChainExpression(chain) = expr else {
        return None;
    };
    match &chain.expression {
        ChainElement::StaticMemberExpression(m) => chain_link(&m.object, m.optional, &*m, scoping)
            .or_else(|| chain_descend(&m.object, scoping)),
        ChainElement::ComputedMemberExpression(m) => {
            chain_link(&m.object, m.optional, &*m, scoping)
                .or_else(|| chain_descend(&m.object, scoping))
        }
        ChainElement::CallExpression(call) => {
            chain_link(&call.callee, call.optional, &*call, scoping)
                .or_else(|| chain_descend(&call.callee, scoping))
        }
        _ => None,
    }
}

pub(super) fn chain_link<T>(
    object: &Expression,
    optional: bool,
    _node: &T,
    scoping: &Scoping,
) -> Option<SymbolId> {
    if !optional {
        return None;
    }
    match skip_parens_expr(object) {
        Expression::Identifier(ident) => resolve_symbol_id(ident, scoping),
        _ => None,
    }
}

// Down through `.a`, `[k]` and `(...)` links to the one whose object is the root.
pub(super) fn chain_descend(expr: &Expression, scoping: &Scoping) -> Option<SymbolId> {
    match skip_parens_expr(expr) {
        Expression::StaticMemberExpression(m) => chain_link(&m.object, m.optional, &**m, scoping)
            .or_else(|| chain_descend(&m.object, scoping)),
        Expression::ComputedMemberExpression(m) => chain_link(&m.object, m.optional, &**m, scoping)
            .or_else(|| chain_descend(&m.object, scoping)),
        Expression::CallExpression(call) => {
            chain_link(&call.callee, call.optional, &**call, scoping)
                .or_else(|| chain_descend(&call.callee, scoping))
        }
        _ => None,
    }
}

pub(super) fn skip_parens_expr<'a, 'b>(mut expr: &'b Expression<'a>) -> &'b Expression<'a> {
    while let Expression::ParenthesizedExpression(inner) = expr {
        expr = &inner.expression;
    }
    expr
}

pub(super) fn chain_equality<'a, 'b>(
    chain: &'b Expression<'a>,
    other: &'b Expression<'a>,
    scoping: &Scoping,
) -> Option<(SymbolId, &'b Expression<'a>)> {
    chain_root(skip_parens_expr(chain), scoping).map(|symbol| (symbol, other))
}

pub(super) fn may_be_undefined(arena: &TypeArena, id: TypeId, loose: bool) -> bool {
    match arena.get(id) {
        Type::Undefined | Type::Any | Type::Unknown | Type::Void => true,
        Type::Null => loose,
        Type::Union(members) => members.iter().any(|&m| may_be_undefined(arena, m, loose)),
        _ => false,
    }
}

pub(super) fn by_symbol(
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
