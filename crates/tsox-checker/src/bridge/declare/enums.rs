//! Enum declaration: member resolution, constant evaluation, merged shapes.
use super::*;

pub(in crate::bridge) fn declare_enum(
    decl: &oxc_ast::ast::TSEnumDeclaration,
    ctx: &mut CheckContext<'_, '_>,
) {
    let Some(symbol_id) = decl.id.symbol_id.get() else {
        return;
    };
    // Members of an earlier block of this enum can be named by a later one
    // (`W = Enum1.A0`), so their values seed the evaluation.
    let prior: Vec<(String, f64)> = match ctx.symbols.get(symbol_id) {
        Some(previous) => match ctx.arena.get(previous) {
            Type::Object(object) => object
                .properties
                .iter()
                .filter_map(|property| match ctx.arena.get(property.type_id) {
                    Type::NumberLiteral(value) => Some((property.name.to_string(), *value)),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        },
        None => Vec::new(),
    };
    let Some(members) = resolve_enum_members(decl, &prior, &mut ctx.arena) else {
        return;
    };
    for (name, type_id) in &members {
        let value = match ctx.arena.get(*type_id) {
            Type::NumberLiteral(n) => tsox_model::ConstValue::number(*n),
            Type::StringLiteral(s) => tsox_model::ConstValue::String(s.as_str().into()),
            _ => continue,
        };
        ctx.enum_values
            .push((decl.id.name.as_str().into(), name.as_str().into(), value));
    }

    // A TypeScript enum is really two separate names sharing one identifier: used
    // in a type position, it means the union of its member literal types; used in
    // a value position, it means an object whose properties are its members. Both
    // are built from the same resolved member list. Ambient enum declarations can
    // merge when their members do not collide, so preserve both halves.
    // Only an enum of the same scope merges: a top-level `enum A` is not the previous block of
    // `A` inside `namespace N`.
    let previous_enum = match ctx.namespace.resolve(&decl.id.name, &mut ctx.arena) {
        crate::namespace::Resolution::Resolved(existing)
            if ctx.arena.is_enum(existing)
                && ctx.namespace.is_declared_in_current_scope(&decl.id.name) =>
        {
            Some(existing)
        }
        _ => None,
    };
    let (mut type_position, mut value_type) = ctx.arena.build_enum(&decl.id.name, members);
    if let Some(previous) = previous_enum {
        let body = ctx.arena.alloc_union(vec![previous, type_position]);
        let name = ctx.arena.alloc_name(decl.id.name.to_string());
        type_position = ctx.arena.alloc_named(name, body);
        ctx.arena.mark_enum(type_position);
        if let Some(previous_value) = ctx.symbols.get(symbol_id) {
            if let Some(merged) =
                merge_enum_value_shapes(&mut ctx.arena, &decl.id.name, previous_value, value_type)
            {
                value_type = merged;
            }
        }
    }
    ctx.namespace
        .insert_enum(&decl.id.name, type_position, decl.id.span);
    ctx.declare_value(symbol_id, &decl.id.name, value_type);
    let spaces = ctx.symbol_capabilities.entry(symbol_id).or_default();
    spaces.has_type = true;
    spaces.has_value = true;
    spaces.value_available = true;
}

pub(in crate::bridge) fn merge_enum_value_shapes(
    arena: &mut crate::arena::TypeArena,
    name: &str,
    previous: crate::arena::TypeId,
    incoming: crate::arena::TypeId,
) -> Option<crate::arena::TypeId> {
    let mut properties = match arena.get(previous) {
        Type::Object(object) => object.properties.to_vec(),
        _ => return None,
    };
    let incoming = match arena.get(incoming) {
        Type::Object(object) => object.properties.to_vec(),
        _ => return None,
    };
    for property in incoming {
        if !properties
            .iter()
            .any(|existing| existing.name == property.name)
        {
            properties.push(property);
        }
    }
    let object = arena.alloc(Type::Object(crate::types::ObjectType::new(properties)));
    let name = arena.alloc_name(format!("typeof {name}"));
    Some(arena.alloc_named(name, object))
}

pub(in crate::bridge) fn resolve_enum_members(
    decl: &oxc_ast::ast::TSEnumDeclaration,
    prior: &[(String, f64)],
    arena: &mut crate::arena::TypeArena,
) -> Option<Vec<(String, crate::arena::TypeId)>> {
    use oxc_ast::ast::TSEnumMemberName;

    let mut members = Vec::with_capacity(decl.body.members.len());
    // Every numeric member's value so far, by name, so a later initializer can
    // refer to an earlier member (`ReadWrite = Read | Write`).
    let mut numeric_values: Vec<(String, f64)> = prior.to_vec();
    let enum_name = decl.id.name.as_str();
    // Tracks the running value for TypeScript's own auto-increment rule: a
    // numeric member with no initializer takes the previous numeric member's
    // value plus one. Reset to None whenever a string member breaks that chain,
    // since auto-increment only continues across consecutive numeric members.
    let mut prev_numeric: Option<f64> = None;

    for member in &decl.body.members {
        let name = match &member.id {
            TSEnumMemberName::Identifier(id) => id.name.to_string(),
            TSEnumMemberName::String(s) => s.value.to_string(),

            TSEnumMemberName::ComputedString(_) | TSEnumMemberName::ComputedTemplateString(_) => {
                // A computed member name means the enum's shape cannot be known
                // without evaluating an expression, so the whole enum is left
                // unresolved rather than guessing.
                return None;
            }
        };

        let member_type = match &member.initializer {
            Some(Expression::StringLiteral(s)) => {
                prev_numeric = None;
                arena.alloc(Type::StringLiteral(s.value.to_string()))
            }

            // Any other initializer is only usable if it is a constant
            // expression this checker can fully evaluate, as tsc does for
            // `1 << 0` or `Read | Write`. One it cannot evaluate, such as a
            // function call, leaves the whole enum unresolved rather than
            // guessing a value.
            Some(initializer) => {
                let value = eval_enum_constant(initializer, enum_name, &numeric_values)?;
                prev_numeric = Some(value);
                numeric_values.push((name.clone(), value));
                arena.alloc(Type::NumberLiteral(value))
            }
            None => {
                let next = match prev_numeric {
                    Some(n) => n + 1.0,
                    None if members.is_empty() => 0.0,

                    None => return None,
                };
                prev_numeric = Some(next);
                numeric_values.push((name.clone(), next));
                arena.alloc(Type::NumberLiteral(next))
            }
        };

        members.push((name, member_type));
    }

    Some(members)
}

// Evaluates the constant numeric expressions tsc accepts as an enum member
// initializer: numeric literals, parentheses, unary + - ~, the arithmetic and
// bitwise binary operators, and references to earlier members of the same enum.
// Anything else, or a result that is not a finite number, is None, which leaves
// the enum unresolved the same way an unsupported initializer always has.
pub(in crate::bridge) fn eval_enum_constant(
    expr: &Expression,
    enum_name: &str,
    known: &[(String, f64)],
) -> Option<f64> {
    let value = match expr {
        Expression::NumericLiteral(n) => n.value,
        Expression::ParenthesizedExpression(inner) => {
            eval_enum_constant(&inner.expression, enum_name, known)?
        }
        Expression::Identifier(id) => known.iter().find(|(name, _)| name == id.name.as_str())?.1,
        // `Enum.Member`, `Enum["Member"]` and ``Enum[`Member`]`` name a member of this enum.
        Expression::StaticMemberExpression(member) => {
            let Expression::Identifier(object) = &member.object else {
                return None;
            };
            if object.name.as_str() != enum_name {
                return None;
            }
            known
                .iter()
                .find(|(name, _)| name == member.property.name.as_str())?
                .1
        }
        Expression::ComputedMemberExpression(member) => {
            let Expression::Identifier(object) = &member.object else {
                return None;
            };
            if object.name.as_str() != enum_name {
                return None;
            }
            let key = match &member.expression {
                Expression::StringLiteral(s) => s.value.to_string(),
                Expression::TemplateLiteral(t)
                    if t.expressions.is_empty() && t.quasis.len() == 1 =>
                {
                    t.quasis[0].value.cooked.as_ref()?.to_string()
                }
                _ => return None,
            };
            known.iter().find(|(name, _)| *name == key)?.1
        }
        Expression::UnaryExpression(unary) => {
            let operand = eval_enum_constant(&unary.argument, enum_name, known)?;
            match unary.operator {
                UnaryOperator::UnaryPlus => operand,
                UnaryOperator::UnaryNegation => -operand,
                UnaryOperator::BitwiseNot => f64::from(!to_int32(operand)),
                _ => return None,
            }
        }
        Expression::BinaryExpression(binary) => {
            let left = eval_enum_constant(&binary.left, enum_name, known)?;
            let right = eval_enum_constant(&binary.right, enum_name, known)?;
            match binary.operator {
                BinaryOperator::Addition => left + right,
                BinaryOperator::Subtraction => left - right,
                BinaryOperator::Multiplication => left * right,
                BinaryOperator::Division => left / right,
                BinaryOperator::Remainder => left % right,
                BinaryOperator::Exponential => left.powf(right),
                BinaryOperator::ShiftLeft => {
                    f64::from(to_int32(left).wrapping_shl(to_uint32(right) & 31))
                }
                BinaryOperator::ShiftRight => {
                    f64::from(to_int32(left).wrapping_shr(to_uint32(right) & 31))
                }
                BinaryOperator::ShiftRightZeroFill => {
                    f64::from(to_uint32(left).wrapping_shr(to_uint32(right) & 31))
                }
                BinaryOperator::BitwiseAnd => f64::from(to_int32(left) & to_int32(right)),
                BinaryOperator::BitwiseOR => f64::from(to_int32(left) | to_int32(right)),
                BinaryOperator::BitwiseXOR => f64::from(to_int32(left) ^ to_int32(right)),
                _ => return None,
            }
        }
        _ => return None,
    };
    value.is_finite().then_some(value)
}

// JavaScript's ToUint32 and ToInt32: truncate, then wrap modulo 2^32. The bitwise
// operators and shifts are defined on these 32-bit views of a number, not on the
// f64 itself.
pub(in crate::bridge) fn to_uint32(n: f64) -> u32 {
    if !n.is_finite() {
        return 0;
    }
    n.trunc().rem_euclid(4_294_967_296.0) as u32
}

pub(in crate::bridge) fn to_int32(n: f64) -> i32 {
    to_uint32(n) as i32
}
