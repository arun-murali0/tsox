// "Did you mean" for a property that does not exist (TS2551): the closest name among the
// type's own members, found the way tsc finds it (`getSpellingSuggestion`): a weighted edit
// distance in which a deletion or insertion costs 1, a substitution 2 and a change of case
// 0.1, accepted when it is under 40% of the name's length.

use crate::arena::TypeArena;
use crate::arena::TypeId;
use crate::types::Type;

const OBJECT_MEMBERS: &[&str] = &[
    "constructor",
    "hasOwnProperty",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toLocaleString",
    "toString",
    "valueOf",
];

const STRING_MEMBERS: &[&str] = &[
    "length",
    "charAt",
    "charCodeAt",
    "concat",
    "indexOf",
    "lastIndexOf",
    "localeCompare",
    "match",
    "replace",
    "search",
    "slice",
    "split",
    "substring",
    "toLowerCase",
    "toLocaleLowerCase",
    "toUpperCase",
    "toLocaleUpperCase",
    "trim",
    "substr",
    "codePointAt",
    "includes",
    "endsWith",
    "normalize",
    "repeat",
    "startsWith",
    "anchor",
    "big",
    "blink",
    "bold",
    "fixed",
    "fontcolor",
    "fontsize",
    "italics",
    "link",
    "small",
    "strike",
    "sub",
    "sup",
    "padStart",
    "padEnd",
    "trimEnd",
    "trimStart",
    "trimLeft",
    "trimRight",
    "matchAll",
    "replaceAll",
    "at",
    "isWellFormed",
    "toWellFormed",
];

const NUMBER_MEMBERS: &[&str] = &["toFixed", "toExponential", "toPrecision"];

const ARRAY_MEMBERS: &[&str] = &[
    "length",
    "pop",
    "push",
    "concat",
    "join",
    "reverse",
    "shift",
    "slice",
    "sort",
    "splice",
    "unshift",
    "indexOf",
    "lastIndexOf",
    "every",
    "some",
    "forEach",
    "map",
    "filter",
    "reduce",
    "reduceRight",
    "find",
    "findIndex",
    "fill",
    "copyWithin",
    "entries",
    "keys",
    "values",
    "includes",
    "flatMap",
    "flat",
    "at",
    "findLast",
    "findLastIndex",
    "toReversed",
    "toSorted",
    "toSpliced",
    "with",
];

fn levenshtein_with_max(left: &[char], right: &[char], max: f64) -> Option<f64> {
    let big = max + 0.01;
    let mut previous: Vec<f64> = (0..=right.len()).map(|n| n as f64).collect();
    let mut current = vec![0.0; right.len() + 1];
    for i in 1..=left.len() {
        let c1 = left[i - 1];
        current[0] = i as f64;
        let mut column_min = i as f64;
        for j in 1..=right.len() {
            let c2 = right[j - 1];
            let substitution = if c1.to_lowercase().eq(c2.to_lowercase()) {
                previous[j - 1] + 0.1
            } else {
                previous[j - 1] + 2.0
            };
            let distance = if c1 == c2 {
                previous[j - 1]
            } else {
                (previous[j] + 1.0)
                    .min(current[j - 1] + 1.0)
                    .min(substitution)
            };
            current[j] = distance;
            column_min = column_min.min(distance);
        }
        if column_min > max {
            return None;
        }
        std::mem::swap(&mut previous, &mut current);
        let _ = big;
    }
    let result = previous[right.len()];
    (result <= max).then_some(result)
}

pub fn suggest<'a>(name: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let name_chars: Vec<char> = name.chars().collect();
    let maximum_length_difference = 2.max((name_chars.len() as f64 * 0.34) as usize);
    let mut best_distance = (name_chars.len() as f64 * 0.4).floor() + 1.0;
    let mut best: Option<String> = None;
    for candidate in candidates {
        let candidate_chars: Vec<char> = candidate.chars().collect();
        if candidate_chars.len().abs_diff(name_chars.len()) > maximum_length_difference
            || candidate == name
        {
            continue;
        }
        if candidate_chars.len() < 3 && candidate.to_lowercase() != name.to_lowercase() {
            continue;
        }
        if let Some(distance) =
            levenshtein_with_max(&name_chars, &candidate_chars, best_distance - 0.1)
        {
            best_distance = distance;
            best = Some(candidate.to_string());
        }
    }
    best
}

/// The names a property of `type_id` could have been meant to be, or none when the type's
/// members are not all known here.
pub fn member_names(arena: &TypeArena, type_id: TypeId) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let builtin: &[&str] = match arena.get(type_id) {
        Type::String | Type::StringLiteral(_) => STRING_MEMBERS,
        Type::Number | Type::NumberLiteral(_) => NUMBER_MEMBERS,
        Type::Array(_) | Type::Tuple(_) => ARRAY_MEMBERS,
        Type::Object(object) => {
            names.extend(object.properties.iter().map(|p| p.name.to_string()));
            &[]
        }
        _ => return Vec::new(),
    };
    names.extend(builtin.iter().map(|name| name.to_string()));
    names.extend(OBJECT_MEMBERS.iter().map(|name| name.to_string()));
    names
}
