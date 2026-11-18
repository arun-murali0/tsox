// Owns: narrowing by `switch` cases and the default clause, and the discriminant of a switch.

use super::*;

// The discriminant shapes a `switch` can narrow on: `kind`, `shape.kind` (the
// discriminated-union form) and `typeof x`. The narrowing overlay is keyed by a
// single symbol, so a discriminant that does not resolve to one (a call, an index,
// a longer chain) has nowhere to record a result and narrows nothing.
pub enum SwitchDiscriminant {
    Identifier(SymbolId),
    Property(SymbolId, String),
    Typeof(SymbolId),
    // `switch (true)`: each clause's test is a condition, narrowing as `if` would.
    True,
}

pub fn switch_discriminant(expr: &Expression, scoping: &Scoping) -> Option<SwitchDiscriminant> {
    match expr {
        Expression::ParenthesizedExpression(inner) => {
            switch_discriminant(&inner.expression, scoping)
        }
        Expression::BooleanLiteral(literal) if literal.value => Some(SwitchDiscriminant::True),
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

pub(super) fn current_type(ctx: &CheckContext<'_, '_>, symbol_id: SymbolId) -> Option<TypeId> {
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
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> NarrowState {
    if matches!(discriminant, SwitchDiscriminant::True) {
        // A body reached through one clause test holds what the test establishes. Several
        // grouped tests would need the union of their states, which is left unnarrowed.
        return match tests {
            [test] => narrow_condition(test, scoping, ctx).0,
            _ => NarrowState::new(),
        };
    }
    let state = narrow_switch_case_inner(discriminant, tests, ctx);
    propagate_destructured(ctx, state)
}

pub(super) fn narrow_switch_case_inner(
    discriminant: &SwitchDiscriminant,
    tests: &[&Expression],
    ctx: &mut CheckContext<'_, '_>,
) -> NarrowState {
    match discriminant {
        SwitchDiscriminant::True => NarrowState::new(),
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
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> NarrowState {
    if matches!(discriminant, SwitchDiscriminant::True) {
        // Every clause test was false, one after the other.
        let saved = ctx.narrow.clone();
        let mut gathered = NarrowState::new();
        for test in tests {
            let (_, when_false) = narrow_condition(test, scoping, ctx);
            ctx.narrow.extend(when_false.clone());
            gathered.extend(when_false);
        }
        ctx.narrow = saved;
        return gathered;
    }
    let state = narrow_switch_default_inner(discriminant, tests, ctx);
    propagate_destructured(ctx, state)
}

pub(super) fn narrow_switch_default_inner(
    discriminant: &SwitchDiscriminant,
    tests: &[&Expression],
    ctx: &mut CheckContext<'_, '_>,
) -> NarrowState {
    match discriminant {
        SwitchDiscriminant::True => NarrowState::new(),
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
