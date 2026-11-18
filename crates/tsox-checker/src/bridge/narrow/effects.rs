//! What a call used as a statement does to the flow after it. An effect looks at the call and
//! answers the narrowing it leaves behind, or `None` when it has none to give. `EFFECTS` is tried
//! in order and every answer is applied, so two effects of one call add up. The two places that
//! see a discarded call (an expression statement, a non-final part of a comma sequence) go through
//! `apply_call_effects`, so the rule lives here once.
use super::*;
use oxc_ast::ast::CallExpression;

pub(super) type EffectFn = for<'a, 'b, 'c> fn(
    &CallExpression<'a>,
    &Scoping,
    &mut CheckContext<'b, 'c>,
) -> Option<NarrowState>;

#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct Effect {
    pub name: &'static str,
    pub doc: &'static str,
    pub run: EffectFn,
}

pub(super) const EFFECTS: &[Effect] = &[Effect {
    name: "assertion_predicate",
    doc: "`assertFoo(x);` where the callee says `asserts x [is T]`: x is narrowed from here on.",
    run: assertion_predicate,
}];

fn assertion_predicate(
    call: &CallExpression<'_>,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> Option<NarrowState> {
    narrow_by_predicate_call(call, scoping, ctx, true).map(|(narrowed, _)| narrowed)
}

/// Applies the effects of `expression` when it is a call (parentheses allowed) whose value is
/// discarded. Anything else is left alone.
pub fn apply_call_effects(
    expression: &Expression<'_>,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) {
    let mut expression = expression;
    while let Expression::ParenthesizedExpression(inner) = expression {
        expression = &inner.expression;
    }
    let Expression::CallExpression(call) = expression else {
        return;
    };
    for effect in EFFECTS {
        if let Some(narrowed) = (effect.run)(call, scoping, ctx) {
            ctx.narrow.extend(narrowed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effects_have_unique_names_and_docs() {
        let mut seen = std::collections::HashSet::new();
        for effect in EFFECTS {
            assert!(seen.insert(effect.name), "duplicate effect {}", effect.name);
            assert!(!effect.doc.is_empty());
        }
    }
}
