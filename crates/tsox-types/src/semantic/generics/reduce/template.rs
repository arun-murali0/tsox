//! Template literal types and string mapping types (`Uppercase<T>` and friends).
use super::*;

pub fn apply_string_case(case: crate::types::StringCase, text: &str) -> String {
    use crate::types::StringCase::*;
    match case {
        Upper => text.to_uppercase(),
        Lower => text.to_lowercase(),
        Capitalize | Uncapitalize => {
            let mut chars = text.chars();
            match chars.next() {
                Some(first) => {
                    let head: String = if case == Capitalize {
                        first.to_uppercase().collect()
                    } else {
                        first.to_lowercase().collect()
                    };
                    head + chars.as_str()
                }
                None => String::new(),
            }
        }
    }
}

/// `Uppercase<S>` and friends: applied to every literal of `operand`, kept symbolic for
/// anything else.
pub fn reduce_string_mapping(
    arena: &mut TypeArena,
    case: crate::types::StringCase,
    operand: TypeId,
) -> TypeId {
    match arena.get(operand).clone() {
        Type::StringLiteral(text) => {
            arena.alloc(Type::StringLiteral(apply_string_case(case, &text)))
        }
        Type::Union(members) => {
            let mapped: Vec<TypeId> = members
                .into_iter()
                .map(|member| reduce_string_mapping(arena, case, member))
                .collect();
            arena.alloc_union(mapped)
        }
        Type::Never | Type::Any => operand,
        _ => arena.alloc(Type::StringMapping(case, operand)),
    }
}

pub(super) fn literal_texts(arena: &TypeArena, id: TypeId, out: &mut Vec<String>) -> Option<()> {
    match arena.get(id) {
        Type::StringLiteral(text) => out.push(text.clone()),
        Type::NumberLiteral(value) => out.push(if value.fract() == 0.0 && value.abs() < 1e15 {
            format!("{}", *value as i64)
        } else {
            format!("{value}")
        }),
        Type::BooleanLiteral(value) => out.push(value.to_string()),
        Type::Null => out.push("null".into()),
        Type::Undefined => out.push("undefined".into()),
        Type::Union(members) => {
            for &member in members {
                literal_texts(arena, member, out)?;
            }
        }
        _ => return None,
    }
    Some(())
}

/// `` `a${X}b` ``: every placeholder a literal (or union of literals) gives the union of the
/// strings; any other placeholder keeps the type as a pattern.
pub fn reduce_template(arena: &mut TypeArena, texts: Vec<String>, types: Vec<TypeId>) -> TypeId {
    let mut options: Vec<Vec<String>> = Vec::with_capacity(types.len());
    let mut all_literal = true;
    for &part in &types {
        if matches!(arena.get(part), Type::Never) {
            return arena.never();
        }
        let mut list = Vec::new();
        if literal_texts(arena, part, &mut list).is_none() {
            all_literal = false;
            break;
        }
        options.push(list);
    }
    let total: usize = options.iter().map(Vec::len).product();
    if all_literal && total <= 10_000 {
        let mut results = vec![texts[0].clone()];
        for (index, list) in options.iter().enumerate() {
            let mut next = Vec::with_capacity(results.len() * list.len());
            for prefix in &results {
                for item in list {
                    next.push(format!("{prefix}{item}{}", texts[index + 1]));
                }
            }
            results = next;
        }
        let members: Vec<TypeId> = results
            .into_iter()
            .map(|text| arena.alloc(Type::StringLiteral(text)))
            .collect();
        return arena.alloc_union(members);
    }
    arena.alloc(Type::Template(crate::types::TemplateType { texts, types }))
}

/// The text each placeholder of `template` covers when `text` matches it, shortest
/// match first for every placeholder but the last.
pub fn match_template(
    arena: &TypeArena,
    template: &crate::types::TemplateType,
    text: &str,
) -> Option<Vec<String>> {
    let first = &template.texts[0];
    let mut rest = text.strip_prefix(first.as_str())?;
    let mut captures = Vec::with_capacity(template.types.len());
    for (index, &part) in template.types.iter().enumerate() {
        let next = &template.texts[index + 1];
        let last = index + 1 == template.types.len();
        let capture = if last {
            let body = rest.strip_suffix(next.as_str())?;
            rest = "";
            body
        } else if next.is_empty() {
            // Nothing separates this placeholder from the next: it takes one character.
            let width = rest.chars().next().map_or(0, char::len_utf8);
            let body = &rest[..width];
            rest = &rest[width..];
            body
        } else {
            let at = rest.find(next.as_str())?;
            let body = &rest[..at];
            rest = &rest[at + next.len()..];
            body
        };
        if !placeholder_accepts(arena, part, capture) {
            return None;
        }
        captures.push(capture.to_string());
    }
    rest.is_empty().then_some(captures)
}

pub(super) fn placeholder_accepts(arena: &TypeArena, part: TypeId, capture: &str) -> bool {
    match arena.get(part) {
        Type::Number => capture.parse::<f64>().is_ok() && !capture.is_empty(),
        Type::Boolean => capture == "true" || capture == "false",
        Type::StringLiteral(text) => text == capture,
        Type::StringMapping(case, _) => apply_string_case(*case, capture) == capture,
        _ => true,
    }
}
