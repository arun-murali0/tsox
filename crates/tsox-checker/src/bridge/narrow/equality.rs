// Owns: narrowing by an equality test (`==`, `!=`, `===`, `!==`) on a variable or on a property
// chain. Each form is one rule in `EQUALITY_RULES`; the first rule that recognises the shape
// decides, so the rules never compete. Does not own `in`, `instanceof`, truthiness or the
// `&&` `||` `!` combinators: those are handled in `condition.rs`.
//
// To add a form: write a function with the `EqualityRule` signature, add one entry below, and
// add a fixture. Order is precedence and must be kept: the optional-chain forms come first
// because they also look like plain `typeof` and literal tests.

use super::*;
use oxc_ast::ast::BinaryExpression;

/// What a rule is told about the operator it was reached through.
pub(super) struct EqualityTest {
    /// `!=` or `!==`: the branches swap.
    pub negated: bool,
    /// `==` or `!=`: JavaScript's coercions apply, so only the null and undefined forms narrow.
    pub loose: bool,
}

pub(super) type EqualityRule = for<'a, 'b, 'c> fn(
    &BinaryExpression<'a>,
    &EqualityTest,
    &Scoping,
    &mut CheckContext<'b, 'c>,
) -> Option<(NarrowState, NarrowState)>;

// `name` and `doc` are read by the tests and by the contributor map, not by the checker.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct Rule {
    pub name: &'static str,
    pub doc: &'static str,
    pub run: EqualityRule,
}

pub(super) const EQUALITY_RULES: &[Rule] = &[
    Rule {
        name: "optional_chain_typeof",
        doc: "`typeof o?.foo === \"number\"`: any tag but \"undefined\" means the chain got past `o`.",
        run: optional_chain_typeof,
    },
    Rule {
        name: "optional_chain_equality",
        doc: "`o?.foo === value`: equal to something not nullish means `o` is there.",
        run: optional_chain_equality,
    },
    Rule {
        name: "typeof_tag",
        doc: "`typeof x === \"string\"` narrows `x` by the tag; an unknown tag narrows nothing.",
        run: typeof_tag,
    },
    Rule {
        name: "nullish",
        doc: "`x === null`, `x == undefined` and their negations.",
        run: nullish,
    },
    Rule {
        name: "literal",
        doc: "`x === \"a\"`: strict equality against a literal.",
        run: literal,
    },
    Rule {
        name: "discriminant",
        doc: "`shape.kind === \"circle\"`: narrows `shape` to the union members whose `kind` can be that literal.",
        run: discriminant,
    },
];

/// The pair of overlays (true edge, false edge) an equality test establishes. Empty when no
/// rule recognises the shape.
pub(super) fn narrow_equality(
    bin: &BinaryExpression<'_>,
    negated: bool,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> (NarrowState, NarrowState) {
    let test = EqualityTest {
        negated,
        loose: matches!(
            bin.operator,
            BinaryOperator::Equality | BinaryOperator::Inequality
        ),
    };
    for rule in EQUALITY_RULES {
        if let Some(pair) = (rule.run)(bin, &test, scoping, ctx) {
            return pair;
        }
    }
    empty_pair()
}

fn optional_chain_typeof<'a, 'b>(
    bin: &BinaryExpression<'a>,
    test: &EqualityTest,
    scoping: &Scoping,
    ctx: &mut CheckContext<'b, '_>,
) -> Option<(NarrowState, NarrowState)> {
    let negated = test.negated;
    for (typeof_side, tag_side) in [(&bin.left, &bin.right), (&bin.right, &bin.left)] {
        if let Expression::UnaryExpression(unary) = skip_parens_expr(typeof_side)
            && unary.operator == UnaryOperator::Typeof
            && let Some(symbol_id) = chain_root(skip_parens_expr(&unary.argument), scoping)
            && let Expression::StringLiteral(tag) = skip_parens_expr(tag_side)
        {
            let narrow_true = (tag.value != "undefined") != negated;
            return Some(by_symbol(
                ctx,
                symbol_id,
                move |arena, current, want_true| {
                    if want_true == narrow_true {
                        narrow_to_non_nullish(arena, current)
                    } else {
                        current
                    }
                },
            ));
        }
    }
    None
}

fn optional_chain_equality<'a, 'b>(
    bin: &BinaryExpression<'a>,
    test: &EqualityTest,
    scoping: &Scoping,
    ctx: &mut CheckContext<'b, '_>,
) -> Option<(NarrowState, NarrowState)> {
    let negated = test.negated;
    let (symbol_id, other) = chain_equality(&bin.left, &bin.right, scoping)
        .or_else(|| chain_equality(&bin.right, &bin.left, scoping))?;
    let nullish_literal = match other {
        Expression::NullLiteral(_) => true,
        Expression::Identifier(id) => id.name == "undefined",
        _ => false,
    };
    let on_equal = if nullish_literal {
        Some(false)
    } else {
        let before = ctx.diagnostics.len();
        let other_type = crate::bridge::expressions::infer_expression_type(other, scoping, ctx);
        ctx.diagnostics.truncate(before);
        // `===` with `null` still means the chain produced a value; only `undefined`
        // (or a type that might be it) can equal a chain that stopped early.
        (!may_be_undefined(&ctx.arena, other_type, test.loose)).then_some(true)
    };
    let on_equal = on_equal?;
    // Whether the true edge is the one that gets the narrowing.
    let narrow_true = on_equal != negated;
    Some(by_symbol(
        ctx,
        symbol_id,
        move |arena, current, want_true| {
            if want_true == narrow_true {
                narrow_to_non_nullish(arena, current)
            } else {
                current
            }
        },
    ))
}

fn typeof_tag<'a, 'b>(
    bin: &BinaryExpression<'a>,
    test: &EqualityTest,
    scoping: &Scoping,
    ctx: &mut CheckContext<'b, '_>,
) -> Option<(NarrowState, NarrowState)> {
    let negated = test.negated;
    let (symbol_id, tag) = typeof_check(&bin.left, &bin.right, scoping)
        .or_else(|| typeof_check(&bin.right, &bin.left, scoping))?;
    // A tag no type in this checker can ever match, "bigint" or "symbol", would filter
    // every union member out and leave never in the true branch, turning a gap in the
    // type model into errors on the source. Narrowing nothing is less precise but never
    // wrong.
    if !is_known_typeof_tag(&tag) {
        return Some(empty_pair());
    }
    Some(by_symbol(ctx, symbol_id, |arena, current, want_true| {
        narrow_by_typeof(arena, current, &tag, want_true != negated)
    }))
}

fn nullish<'a, 'b>(
    bin: &BinaryExpression<'a>,
    test: &EqualityTest,
    scoping: &Scoping,
    ctx: &mut CheckContext<'b, '_>,
) -> Option<(NarrowState, NarrowState)> {
    let (negated, is_loose) = (test.negated, test.loose);
    let (symbol_id, is_null) = nullish_check(&bin.left, &bin.right, scoping)
        .or_else(|| nullish_check(&bin.right, &bin.left, scoping))?;
    Some(by_symbol(ctx, symbol_id, |arena, current, want_true| {
        narrow_by_nullish(arena, current, is_null, is_loose, want_true != negated)
    }))
}

// Loose equality against a literal (x == "a") is left alone: unlike null/undefined,
// JavaScript's abstract-equality coercions for other literals (0 == "0", "" == false) mean a
// matching member of x's own union is not the only way this can pass, so narrowing to it could
// remove a value the true branch can still actually hold.
fn literal<'a, 'b>(
    bin: &BinaryExpression<'a>,
    test: &EqualityTest,
    scoping: &Scoping,
    ctx: &mut CheckContext<'b, '_>,
) -> Option<(NarrowState, NarrowState)> {
    if test.loose {
        return None;
    }
    let negated = test.negated;
    let (symbol_id, literal) = literal_check(&bin.left, &bin.right, scoping, &mut ctx.arena)
        .or_else(|| literal_check(&bin.right, &bin.left, scoping, &mut ctx.arena))?;
    Some(by_symbol(
        ctx,
        symbol_id,
        move |arena, current, want_true| {
            narrow_by_literal(arena, current, literal, want_true != negated)
        },
    ))
}

// `shape.kind === "circle"`: narrows `shape` itself, when it is a union of object types, to the
// members whose `kind` can be that literal. Strict equality only, for the same coercion reason
// as above.
fn discriminant<'a, 'b>(
    bin: &BinaryExpression<'a>,
    test: &EqualityTest,
    scoping: &Scoping,
    ctx: &mut CheckContext<'b, '_>,
) -> Option<(NarrowState, NarrowState)> {
    if test.loose {
        return None;
    }
    let negated = test.negated;
    let (symbol_id, property, literal) =
        member_literal_check(&bin.left, &bin.right, scoping, &mut ctx.arena)
            .or_else(|| member_literal_check(&bin.right, &bin.left, scoping, &mut ctx.arena))?;
    Some(by_symbol(
        ctx,
        symbol_id,
        move |arena, current, want_true| {
            narrow_by_property_literals(arena, current, &property, &[literal], want_true != negated)
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::EQUALITY_RULES;

    #[test]
    fn rules_have_unique_names_and_docs_and_the_chain_forms_come_first() {
        let names: Vec<&str> = EQUALITY_RULES.iter().map(|rule| rule.name).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len());
        assert!(EQUALITY_RULES.iter().all(|rule| !rule.doc.is_empty()));
        assert_eq!(
            &names[..2],
            ["optional_chain_typeof", "optional_chain_equality"]
        );
    }
}
