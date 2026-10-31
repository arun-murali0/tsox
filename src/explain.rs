//! Why a type is not assignable to another, in the shape tsc reports it.
//!
//! tsc does not print a bare "Type 'A' is not assignable to type 'B'." It prints a
//! chain: that line, then the reason it holds one level down (a union member that
//! fails, a property that is missing or has the wrong type, a parameter that is not
//! accepted), each level indented two spaces under the one above.
//!
//! The chain is built only after `is_subtype` has already said "no", so nothing here
//! decides assignability. A level that cannot be worked out is left off, which makes
//! the message shorter than tsc's and never wrong about whether there is an error.
//!
//! Rules this follows, each checked against `tsc` itself:
//!
//! - A union source names its first failing member. tsc keeps union members in type-id
//!   order, in which `undefined` and `null` come before `string`, then `number`,
//!   `boolean`, then everything else in creation order. That is why `number | null`
//!   against `number` names `null`, whatever order the union is printed in.
//! - A union target is not elaborated.
//! - Arrays and generic applications of the same declaration name the first element or
//!   type argument that fails, and nothing about the structure in between.
//! - A missing property is reported as one line (`Property 'y' is missing in type ...`)
//!   for one, or as a list for several (five at most are named; with more, the first
//!   four and a count). When that line is the only reason, the "is not assignable"
//!   line above it is dropped, so it becomes the top message and takes its code.
//! - A property of the wrong type adds `Types of property 'p' are incompatible.`, and a
//!   parameter adds `Types of parameters 'a' and 'b' are incompatible.` (the target's
//!   parameter type is the one that must be assignable to the source's).

use crate::arena::{TypeArena, TypeId};
use crate::diagnostic_codes::DiagnosticCode;
use crate::subtyping::{is_subtype, param_type_at, property_relates};
use crate::type_display::{display_source_type, display_type};
use crate::types::{FunctionType, ObjectType, Type};

// A chain this deep is a recursive type being walked; tsc itself stops elaborating
// long before this.
const MAX_DEPTH: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    NotAssignable,
    Missing(DiagnosticCode),
    Detail,
}

/// One line of a chain.
#[derive(Clone, Debug)]
pub struct Level {
    kind: Kind,
    text: String,
}

impl Level {
    fn not_assignable(arena: &TypeArena, source: TypeId, target: TypeId) -> Self {
        let shown_target = Self::non_nullable_target(arena, source, target);
        Self {
            kind: Kind::NotAssignable,
            text: format!(
                "Type '{}' is not assignable to type '{}'.",
                display_source_type(arena, source, shown_target),
                display_type(arena, shown_target)
            ),
        }
    }

    fn detail(text: String) -> Self {
        Self {
            kind: Kind::Detail,
            text,
        }
    }

    fn is_missing(&self) -> bool {
        matches!(self.kind, Kind::Missing(_))
    }

    /// tsc reports against the one type that is left when a source that is not null or
    /// undefined is compared with `T | null` or `T | undefined`: `"x"` into `Node | null` is
    /// "not assignable to type 'Node'". With more than one type left (`A | B | null`) the
    /// whole union is named, and so it is when the source can itself be null or undefined.
    fn non_nullable_target(arena: &TypeArena, source: TypeId, target: TypeId) -> TypeId {
        let Type::Union(members) = arena.get(target) else {
            return target;
        };
        let nullish = |id: TypeId| matches!(arena.get(id), Type::Null | Type::Undefined);
        let source_nullish = match arena.get(source) {
            Type::Union(parts) => parts.iter().any(|&p| nullish(p)),
            _ => nullish(source),
        };
        if source_nullish {
            return target;
        }
        let mut rest = members.iter().copied().filter(|&m| !nullish(m));
        match (rest.next(), rest.next()) {
            (Some(only), None) if members.iter().any(|&m| nullish(m)) => only,
            _ => target,
        }
    }

    /// A level that opens a message of its own, such as `Argument of type ...`.
    pub fn headline(text: String) -> Self {
        Self::detail(text)
    }
}

/// The whole chain for `source` not being assignable to `target`, head included.
/// When the only reason is a missing property, that line is the head.
pub fn mismatch_levels(arena: &TypeArena, source: TypeId, target: TypeId) -> Vec<Level> {
    reason(arena, source, target, 0)
}

/// Like `mismatch_levels`, for a generic source that was instantiated against the target
/// before the comparison. The reasons come from `reason_source`, the instantiated type,
/// while the first line still names `shown_source`, the type as declared, as tsc does.
pub fn mismatch_levels_via(
    arena: &TypeArena,
    shown_source: TypeId,
    reason_source: TypeId,
    target: TypeId,
) -> Vec<Level> {
    let mut levels = reason(arena, reason_source, target, 0);
    if shown_source != reason_source
        && let Some(first) = levels.first_mut()
        && first.kind == Kind::NotAssignable
    {
        *first = Level::not_assignable(arena, shown_source, target);
    }
    levels
}

/// The reasons under a head the caller writes itself (`Argument of type ...`,
/// `Type ... does not satisfy the constraint ...`), without the "is not assignable"
/// line that head replaces.
pub fn mismatch_children(arena: &TypeArena, source: TypeId, target: TypeId) -> Vec<Level> {
    children(arena, source, target, 0)
}

/// The code of the top line: a missing-property line carries its own, anything else
/// is the code the caller reports a plain mismatch under.
pub fn top_code(levels: &[Level], plain: DiagnosticCode) -> DiagnosticCode {
    match levels.first().map(|level| level.kind) {
        Some(Kind::Missing(code)) => code,
        _ => plain,
    }
}

/// tsc's flattening: each level on its own line, two spaces deeper than the last.
pub fn render(levels: &[Level]) -> String {
    let mut out = String::new();
    for (depth, level) in levels.iter().enumerate() {
        if depth > 0 {
            out.push('\n');
            for _ in 0..depth {
                out.push_str("  ");
            }
        }
        out.push_str(&level.text);
    }
    out
}

fn reason(arena: &TypeArena, source: TypeId, target: TypeId, depth: usize) -> Vec<Level> {
    let below = children(arena, source, target, depth);
    // A lone missing property is promoted to the head (tsc's TS2741) only for a plain
    // object target. For `A & B` tsc keeps "not assignable to 'A & B'" as the head and puts
    // the missing property beneath it, so the promotion is skipped there.
    // Nor is it promoted past an array: `I1[]` to `C1[]` stays "not assignable to 'C1[]'" with
    // the element's missing property beneath, where promoting it named the element instead.
    let target_is_object = matches!(arena.get(target), Type::Object(_));
    if target_is_object && below.first().is_some_and(Level::is_missing) {
        return below;
    }
    let mut levels = vec![Level::not_assignable(arena, source, target)];
    levels.extend(below);
    levels
}

// tsc's line under a tuple mismatch caused by length, in tsc's own order (structuredTypeRelatedTo,
// for a tuple target). `from` is an array or a tuple, given as its elements; an array is one
// rest element. Each rule is tried in turn and the first that fires is the whole reason:
//   1. a source with no rest element that has fewer elements than the target requires,
//   2. a source that requires more elements than a rest-less target has room for,
//   3. a rest-less target against a source that may have a different length: it requires
//      more than the source is sure to have, or it allows fewer than the source may have.
// None of them fires when only the elements differ; that is `tuple_element_reason`.
fn tuple_arity_reason(
    from: &[crate::types::TupleElement],
    to: &[crate::types::TupleElement],
) -> Vec<Level> {
    let min_length = |elements: &[crate::types::TupleElement]| {
        elements.iter().filter(|e| !e.optional && !e.rest).count()
    };
    let (source_len, source_min) = (from.len(), min_length(from));
    let (target_len, target_min) = (to.len(), min_length(to));
    let source_rest = from.iter().any(|e| e.rest);
    let target_rest = to.iter().any(|e| e.rest);

    if !source_rest && source_len < target_min {
        return vec![Level::detail(format!(
            "Source has {source_len} element(s) but target requires {target_min}."
        ))];
    }
    if !target_rest && target_len < source_min {
        return vec![Level::detail(format!(
            "Source has {source_len} element(s) but target allows only {target_len}."
        ))];
    }
    if !target_rest && (source_rest || target_len < source_len) {
        return vec![Level::detail(if source_min < target_min {
            format!("Target requires {target_min} element(s) but source may have fewer.")
        } else {
            format!("Target allows only {target_len} element(s) but source may have more.")
        })];
    }
    Vec::new()
}

// The first source element that does not fit the target element it lands on, as tsc says it:
// the position on each side, then why the two types differ. Source elements past the target's
// fixed ones all land on its rest element, and tsc names them as one range
// ("positions 1 through 2"). A source with optional or rest elements of its own has no fixed
// positions to name, so it is left with the head line alone.
fn tuple_element_reason(
    arena: &TypeArena,
    from: &[crate::types::TupleElement],
    to: &[crate::types::TupleElement],
    depth: usize,
) -> Vec<Level> {
    if from.iter().any(|e| e.optional || e.rest) {
        return Vec::new();
    }
    let target_rest = to.last().filter(|e| e.rest);
    let target_fixed = to.len() - usize::from(target_rest.is_some());
    for (position, source) in from.iter().enumerate() {
        let (target_position, wanted) = if position < target_fixed {
            (position, to[position].type_id)
        } else if let Some(rest) = target_rest {
            (target_fixed, rest.type_id)
        } else {
            return Vec::new();
        };
        if is_subtype(arena, source.type_id, wanted) {
            continue;
        }
        let in_rest = position >= target_fixed;
        let text = if in_rest && from.len() - target_fixed > 1 {
            format!(
                "Type at positions {target_fixed} through {} in source is not compatible with type at position {target_position} in target.",
                from.len() - 1
            )
        } else {
            format!(
                "Type at position {position} in source is not compatible with type at position {target_position} in target."
            )
        };
        let mut levels = vec![Level::detail(text)];
        levels.extend(reason(arena, source.type_id, wanted, depth + 2));
        return levels;
    }
    Vec::new()
}

// A tuple read as the array it is assignable to: tsc says the union of the element types
// does not fit the array's element, and then which member of that union is the culprit.
fn tuple_as_array_reason(
    arena: &TypeArena,
    from: &[crate::types::TupleElement],
    to_element: TypeId,
    depth: usize,
) -> Vec<Level> {
    let mut members: Vec<TypeId> = Vec::new();
    for element in from {
        if !members
            .iter()
            .any(|&m| display_type(arena, m) == display_type(arena, element.type_id))
        {
            members.push(element.type_id);
        }
    }
    members.sort_by_key(|&member| tsc_rank(arena, member));
    let Some(failing) = members
        .iter()
        .copied()
        .find(|&member| !is_subtype(arena, member, to_element))
    else {
        return Vec::new();
    };
    if members.len() == 1 {
        return reason(arena, failing, to_element, depth + 1);
    }
    let shown = members
        .iter()
        .map(|&m| display_type(arena, m))
        .collect::<Vec<_>>()
        .join(" | ");
    let mut levels = vec![Level::detail(format!(
        "Type '{shown}' is not assignable to type '{}'.",
        display_type(arena, to_element)
    ))];
    levels.extend(reason(arena, failing, to_element, depth + 2));
    levels
}

fn children(arena: &TypeArena, source: TypeId, target: TypeId, depth: usize) -> Vec<Level> {
    if depth >= MAX_DEPTH {
        return Vec::new();
    }
    match (arena.get(source), arena.get(target)) {
        // An enum is a primitive to tsc and a mismatch below it is not elaborated.
        (Type::Union(_), _) if arena.is_enum(source) => Vec::new(),
        // Problem: a mismatch against `A & B` stopped at "not assignable to 'A & B'", where
        // tsc goes on to say which part of the intersection failed.
        // Picked: assignable to an intersection means assignable to every member, so the
        // reason is the first member the source does not satisfy, explained as if that
        // member alone were the target (a plain "not assignable" line, or the missing
        // property it lacks). A union source is left to the arms below, which already
        // explain it member by member.
        (source_type, Type::Intersection(members)) if !matches!(source_type, Type::Union(_)) => {
            match members
                .iter()
                .copied()
                .find(|&member| !crate::subtyping::is_subtype(arena, source, member))
            {
                Some(failing) => reason(arena, source, failing, depth + 1),
                None => Vec::new(),
            }
        }
        // Nothing but the parameter itself (or never/any, which never get here) is
        // assignable to a type parameter, since it could be instantiated as anything.
        // tsc says so, and says which of two things is true of the constraint.
        (_, Type::GenericParameter(_, name, bound)) => {
            let shown = display_source_type(arena, source, target);
            let text = match bound {
                Some(bound) if is_subtype(arena, source, *bound) => format!(
                    "'{shown}' is assignable to the constraint of type '{name}', but '{name}' could be instantiated with a different subtype of constraint '{}'.",
                    display_type(arena, *bound)
                ),
                _ => format!(
                    "'{name}' could be instantiated with an arbitrary type which could be unrelated to '{shown}'."
                ),
            };
            vec![Level::detail(text)]
        }
        (Type::Union(members), _) => {
            // `string | "fallback"` is just `string` once tsc has reduced it, so there
            // is no member to point at.
            if distinct_after_reduction(arena, members) <= 1 {
                return Vec::new();
            }
            let mut ordered = members.clone();
            ordered.sort_by_key(|&member| tsc_rank(arena, member));
            match ordered
                .into_iter()
                .find(|&member| !is_subtype(arena, member, target))
            {
                Some(member) => reason(arena, member, target, depth + 1),
                None => Vec::new(),
            }
        }
        (_, Type::Union(_)) => Vec::new(),
        (Type::Tuple(from), Type::Tuple(to)) => {
            let arity = tuple_arity_reason(from, to);
            if arity.is_empty() {
                tuple_element_reason(arena, from, to, depth)
            } else {
                arity
            }
        }
        // An array has no fixed length, so it is a source with one rest element.
        (Type::Array(element), Type::Tuple(to)) => tuple_arity_reason(
            &[crate::types::TupleElement {
                type_id: *element,
                optional: false,
                rest: true,
            }],
            to,
        ),
        (Type::Tuple(from), Type::Array(to)) => tuple_as_array_reason(arena, from, *to, depth),
        (Type::Array(from), Type::Array(to)) => reason(arena, *from, *to, depth + 1),
        (Type::Object(from), Type::Object(to)) => {
            if let (Some((from_name, from_args)), Some((to_name, to_args))) =
                (arena.app_parts(source), arena.app_parts(target))
                && from_name == to_name
                && from_args.len() == to_args.len()
                && let Some(index) =
                    (0..from_args.len()).find(|&i| !is_subtype(arena, from_args[i], to_args[i]))
            {
                return reason(arena, from_args[index], to_args[index], depth + 1);
            }
            object_children(arena, source, target, from, to, depth)
        }
        (Type::Function(from), Type::Function(to)) => function_children(arena, from, to, depth),
        _ => Vec::new(),
    }
}

// How many members a union has once a literal is absorbed by its own base type.
fn distinct_after_reduction(arena: &TypeArena, members: &[TypeId]) -> usize {
    let has = |wanted: fn(&Type) -> bool| members.iter().any(|&m| wanted(arena.get(m)));
    let (string, number, boolean) = (
        has(|t| matches!(t, Type::String)),
        has(|t| matches!(t, Type::Number)),
        has(|t| matches!(t, Type::Boolean)),
    );
    members
        .iter()
        .filter(|&&m| {
            !matches!(
                (arena.get(m), string, number, boolean),
                (Type::StringLiteral(_), true, _, _)
                    | (Type::NumberLiteral(_), _, true, _)
                    | (Type::BooleanLiteral(_), _, _, true)
            )
        })
        .count()
}

// The order tsc compares union members in, which is the order of their type ids.
fn tsc_rank(arena: &TypeArena, member: TypeId) -> u8 {
    match arena.get(member) {
        Type::Undefined => 0,
        Type::Null => 1,
        Type::String => 2,
        Type::Number => 3,
        Type::Boolean | Type::BooleanLiteral(_) => 4,
        _ => 5,
    }
}

// One level at a time, in the order tsc looks. Missing properties come first and end the
// walk, since that one line says it all. After that, the first property in the target's
// order that is optional where the target needs it, or whose type does not relate; its
// reason continues one level down. A chain names the first reason, never every one.
fn object_children(
    arena: &TypeArena,
    source: TypeId,
    target: TypeId,
    from: &ObjectType,
    to: &ObjectType,
    depth: usize,
) -> Vec<Level> {
    let missing: Vec<&str> = to
        .properties
        .iter()
        .filter(|wanted| {
            !wanted.optional && !from.properties.iter().any(|have| have.name == wanted.name)
        })
        .map(|wanted| &*wanted.name)
        .collect();
    if !missing.is_empty() {
        return vec![missing_level(arena, source, target, &missing)];
    }

    for wanted in to.properties.iter() {
        let Some(have) = from.properties.iter().find(|p| p.name == wanted.name) else {
            continue;
        };
        if have.optional && !wanted.optional {
            return vec![Level::detail(format!(
                "Property '{}' is optional in type '{}' but required in type '{}'.",
                wanted.name,
                display_type(arena, source),
                display_type(arena, target)
            ))];
        }
        if !property_relates(arena, have.type_id, wanted) {
            let mut levels = vec![Level::detail(format!(
                "Types of property '{}' are incompatible.",
                wanted.name
            ))];
            levels.extend(reason(arena, have.type_id, wanted.type_id, depth + 1));
            return levels;
        }
    }
    Vec::new()
}

// The three wordings tsc has for missing properties: one, up to five named, and more than
// five (the first four and a count). Each has its own code, so when this line is the only
// reason and becomes the top message it carries the right one.
fn missing_level(arena: &TypeArena, source: TypeId, target: TypeId, missing: &[&str]) -> Level {
    let (from, to) = (display_type(arena, source), display_type(arena, target));
    match missing {
        [only] => Level {
            kind: Kind::Missing(DiagnosticCode::MissingProperty),
            text: format!(
                "Property '{only}' is missing in type '{from}' but required in type '{to}'."
            ),
        },
        names if names.len() <= 5 => Level {
            kind: Kind::Missing(DiagnosticCode::MissingProperties),
            text: format!(
                "Type '{from}' is missing the following properties from type '{to}': {}",
                names.join(", ")
            ),
        },
        names => Level {
            kind: Kind::Missing(DiagnosticCode::MissingPropertiesMany),
            text: format!(
                "Type '{from}' is missing the following properties from type '{to}': {}, and {} more.",
                names[..4].join(", "),
                names.len() - 4
            ),
        },
    }
}

// The first parameter position, over the longer of the two lists, where the target's
// parameter type does not fit the source's (parameters compare the other way round, see
// the header), and otherwise a return type that does not fit. A position with no type on
// one side is skipped. A parameter with no name is called `argN`.
fn function_children(
    arena: &TypeArena,
    from: &FunctionType,
    to: &FunctionType,
    depth: usize,
) -> Vec<Level> {
    let positions = from.params.len().max(to.params.len());
    for position in 0..positions {
        let (Some(have), Some(wanted)) = (
            param_type_at(arena, &from.params, position),
            param_type_at(arena, &to.params, position),
        ) else {
            continue;
        };
        if is_subtype(arena, wanted, have) {
            continue;
        }
        let name_of = |params: &[crate::types::Param]| {
            params
                .get(position)
                .or(params.last())
                .and_then(|param| param.name.as_deref().map(str::to_owned))
                .unwrap_or_else(|| format!("arg{position}"))
        };
        let mut levels = vec![Level::detail(format!(
            "Types of parameters '{}' and '{}' are incompatible.",
            name_of(&from.params),
            name_of(&to.params)
        ))];
        levels.extend(reason(arena, wanted, have, depth + 1));
        return levels;
    }
    if !is_subtype(arena, from.return_type, to.return_type) {
        return reason(arena, from.return_type, to.return_type, depth + 1);
    }
    Vec::new()
}
