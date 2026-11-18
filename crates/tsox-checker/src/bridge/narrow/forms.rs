// Owns: recognising the syntactic shapes a test can have (`typeof x`, `x === null`, `x === literal`)
// and resolving literals and symbols. It recognises; it does not narrow.

use super::*;

pub(super) fn resolve_all_literals(
    tests: &[&Expression],
    arena: &mut TypeArena,
) -> Option<Vec<TypeId>> {
    let mut literals = Vec::with_capacity(tests.len());
    for test in tests {
        literals.push(resolve_literal(test, arena)?);
    }
    Some(literals)
}

// One tag that is not a known `typeof` result gives up on the whole switch, for the reason
// typeof_check gives: a tag no type here can match would rule out every member and leave
// `never`.
pub(super) fn resolve_all_typeof_tags(tests: &[&Expression]) -> Option<Vec<String>> {
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
pub(super) fn resolve_literal(expr: &Expression, arena: &mut TypeArena) -> Option<TypeId> {
    match expr {
        Expression::StringLiteral(lit) => {
            Some(arena.alloc(Type::StringLiteral(lit.value.to_string())))
        }
        Expression::NumericLiteral(lit) => Some(arena.alloc(Type::NumberLiteral(lit.value))),
        Expression::BooleanLiteral(lit) => Some(arena.alloc(Type::BooleanLiteral(lit.value))),
        // `case null:` and `case undefined:` narrow like any other case.
        Expression::NullLiteral(_) => Some(arena.null()),
        Expression::Identifier(ident) if ident.name == "undefined" => Some(arena.undefined()),
        _ => None,
    }
}

pub fn resolve_symbol_id(ident: &IdentifierReference, scoping: &Scoping) -> Option<SymbolId> {
    ident
        .reference_id
        .get()
        .and_then(|reference_id| scoping.get_reference(reference_id).symbol_id())
}

pub(super) fn typeof_check(
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
pub(super) fn narrowed_identifier<'a, 'b>(
    expr: &'a Expression<'b>,
) -> Option<&'a IdentifierReference<'b>> {
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

pub(super) fn nullish_check(
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
pub(super) fn literal_check(
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
