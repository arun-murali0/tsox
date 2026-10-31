use oxc_ast::ast::Expression;
use oxc_semantic::Scoping;
use oxc_span::{GetSpan, Span};

use crate::arena::{TypeArena, TypeId};
use crate::types::Type;

use super::super::context::CheckContext;
use super::super::narrow::narrow_to_non_nullish;
use super::binary::{entity_name, nullish_members};
use super::infer_expression_type;

// Member access is modelled on Type::Object, on a union of types that all have the
// property, and on `.length` for strings and arrays. `.length` is the only
// String/Array member, deliberately: every other String.prototype and
// Array.prototype method (`.map`, `.slice`, `.indexOf`, ...) would need a whole
// method signature modelled, and the array ones take a callback whose parameter
// types have to be inferred from the element type, which is a feature in its own
// right. `.length` is a fixed property returning `number` on both, so it needs
// neither.
//
// A union is looked through, not rejected: as in tsc, a property is readable on a
// union when every member has it, and its type is the union of the members'
// property types. Without this, `shape.kind` on an un-narrowed discriminated union
// would be reported as missing, so the very read that a `kind` check narrows on
// would be a false error before narrowing ever ran. A member that lacks the
// property makes the whole access an error, reported once on the union rather than
// once per member.
pub(crate) fn infer_member_access_type(
    object_type: TypeId,
    property_name: &str,
    span: Span,
    ctx: &mut CheckContext<'_, '_>,
) -> TypeId {
    // A constrained generic parameter (`T extends HasLength`) exposes its
    // constraint's members inside the function body, the same way tsc treats
    // `T`'s accessible shape as its constraint's shape. An unconstrained `T`
    // has no known members, so the lookup finds nothing and reports it.
    let effective_type = match ctx.arena.get(object_type) {
        Type::GenericParameter(_, _, Some(constraint)) => *constraint,
        _ => object_type,
    };

    match lookup_member(&mut ctx.arena, effective_type, property_name) {
        Some(found) => found,
        None => {
            ctx.error(
                crate::diagnostic_messages::messages::property_does_not_exist(
                    &ctx.arena,
                    property_name,
                    effective_type,
                ),
                span,
            );
            ctx.arena.error()
        }
    }
}

// The type of `property_name` on `type_id`, or None when it is not there. It reports
// nothing itself, so a union can ask every member and let the caller report once.
// `any` and the error type answer with the error type: they are compatible with
// everything, which stops one failure from cascading into more diagnostics.
fn lookup_member(arena: &mut TypeArena, type_id: TypeId, property_name: &str) -> Option<TypeId> {
    let effective = match arena.get(type_id) {
        Type::GenericParameter(_, _, Some(constraint)) => *constraint,
        _ => type_id,
    };

    match arena.get(effective) {
        Type::Any | Type::Error => Some(arena.error()),
        Type::String | Type::StringLiteral(_) | Type::Array(_) if property_name == "length" => {
            Some(arena.number())
        }
        Type::Object(object) => {
            match object.properties.iter().find(|p| *p.name == *property_name) {
                Some(property) => Some(property.type_id),
                // The members every value has come before a Record's value type: tsc
                // looks a name up on the object's own members and on `Object` before it
                // falls back to an index signature, so `rec.hasOwnProperty(k)` on a
                // `Record<string, string>` is the method, not a string.
                // A Promise<T> reads as its empty body here, so it is told apart by the
                // application it came from.
                None => universal_member(arena, effective, property_name)
                    .or_else(|| {
                        if arena.is_promise(effective) {
                            promise_member(arena, property_name)
                        } else {
                            None
                        }
                    })
                    .or_else(|| arena.record_value_type(effective)),
            }
        }
        Type::Union(members) => {
            let members = members.clone();
            let mut found = Vec::with_capacity(members.len());
            for member in members {
                found.push(lookup_member(arena, member, property_name)?);
            }
            Some(arena.alloc_union(found))
        }
        // The opposite of a union: a value of `A & B` is an `A` and a `B`, so the members
        // that have the property all contribute, and its type is the intersection of what
        // they give (LLD 1.13). `{ a: number } & { a: string }` has an `a` of type
        // `never`, and one member alone is enough for the property to exist. One whose
        // members disagree about a discriminant is `never`, which has no properties.
        Type::Intersection(members) => {
            if arena.intersection_reduces_to_never(effective) {
                return None;
            }
            let members = members.clone();
            let mut found = Vec::with_capacity(members.len());
            for member in members {
                if let Some(property_type) = lookup_member(arena, member, property_name) {
                    found.push(property_type);
                }
            }
            match found.len() {
                0 => None,
                1 => Some(found[0]),
                _ => Some(
                    arena
                        .alloc_intersection(found)
                        .unwrap_or_else(|_| arena.error()),
                ),
            }
        }
        // A string has the string methods, and every primitive, function and array has the
        // members every value has.
        Type::String | Type::StringLiteral(_) => string_member(arena, property_name)
            .or_else(|| universal_member(arena, effective, property_name)),
        Type::Array(element) => {
            let element = *element;
            array_member(arena, element, property_name)
                .or_else(|| universal_member(arena, effective, property_name))
        }
        // A tuple has the length it was written with and, past that, every method an array
        // of its elements has: `[string, number]` offers what `(string | number)[]` does.
        Type::Tuple(elements) => {
            let elements = elements.clone();
            if property_name == "length" {
                return Some(tuple_length_type(arena, &elements));
            }
            let element = tuple_element_union(arena, &elements, false);
            array_member(arena, element, property_name)
                .or_else(|| universal_member(arena, effective, property_name))
        }
        Type::Number
        | Type::Boolean
        | Type::NumberLiteral(_)
        | Type::BooleanLiteral(_)
        | Type::Function(_) => universal_member(arena, effective, property_name),
        _ => None,
    }
}

// Problem: `s.startsWith('a')`, `s.split(',')` and the rest of the string methods reported
// "Property does not exist", because a string only had `length`.
// Picked: the methods of tsc's default library, by name. An argument that names a plain string or
// number is typed so a wrong one is rejected as tsc rejects it; one that may be a RegExp, a
// function or either (`split`, `replace`, `match`) is `any`.
// Cost: `match` and `matchAll` return `any` rather than the match array, so what is done with
// their result is unchecked; methods the library only adds for later targets are all offered.
fn string_member(arena: &mut TypeArena, property_name: &str) -> Option<TypeId> {
    use crate::types::{FunctionType, Param};
    // One letter per parameter: s string, n number, a any, and a lowercase letter that is
    // optional in capitals (S, N, A). `*` before a letter makes it a rest parameter.
    // The result letter: s string, n number, b boolean, a any, l string[], u number | undefined,
    // v string | undefined.
    let (params, result) = match property_name {
        "charAt" => ("n", 's'),
        "charCodeAt" => ("n", 'n'),
        "codePointAt" => ("n", 'u'),
        "at" => ("n", 'v'),
        "concat" => ("*s", 's'),
        "indexOf" | "lastIndexOf" => ("sN", 'n'),
        "includes" | "startsWith" | "endsWith" => ("sN", 'b'),
        "slice" => ("NN", 's'),
        "substring" => ("nN", 's'),
        "substr" => ("nN", 's'),
        "split" => ("aN", 'l'),
        "trim" | "trimStart" | "trimEnd" | "trimLeft" | "trimRight" | "toUpperCase"
        | "toLowerCase" | "toLocaleUpperCase" | "toLocaleLowerCase" => ("", 's'),
        "padStart" | "padEnd" => ("nS", 's'),
        "repeat" => ("n", 's'),
        "replace" | "replaceAll" => ("aa", 's'),
        "match" | "matchAll" => ("a", 'a'),
        "search" => ("a", 'n'),
        "localeCompare" => ("s*a", 'n'),
        "normalize" => ("S", 's'),
        _ => return None,
    };
    let any = arena.any();
    let mut list = Vec::new();
    let mut rest = false;
    for letter in params.chars() {
        if letter == '*' {
            rest = true;
            continue;
        }
        let base = match letter.to_ascii_lowercase() {
            's' => arena.string(),
            'n' => arena.number(),
            _ => any,
        };
        let type_id = if rest {
            arena.alloc(Type::Array(base))
        } else {
            base
        };
        list.push(Param {
            type_id,
            optional: letter.is_ascii_uppercase(),
            rest,
            name: None,
        });
        rest = false;
    }
    let undefined = arena.undefined();
    let return_type = match result {
        's' => arena.string(),
        'n' => arena.number(),
        'b' => arena.boolean(),
        'l' => {
            let string = arena.string();
            arena.alloc(Type::Array(string))
        }
        'u' => {
            let number = arena.number();
            arena.alloc_union(vec![number, undefined])
        }
        'v' => {
            let string = arena.string();
            arena.alloc_union(vec![string, undefined])
        }
        _ => any,
    };
    Some(arena.alloc(Type::Function(FunctionType {
        params: list,
        return_type,
        type_params: Vec::new(),
        is_untyped: false,
    })))
}

// Problem: `arr.map(f)`, `arr.push(x)` and every other array method reported "Property does
// not exist", because an array only had `length`: a false error on some of the most used code
// in any project.
// Picked: the methods of tsc's default library, by name, as strings got theirs. A method whose
// real signature needs generic inference (`map`, `reduce`, `flatMap`, `find`, and `filter`,
// which has a type-guard overload) takes callbacks and returns what it can say for certain,
// and `any` otherwise. A method that only moves or reports elements is typed from the element
// type, so `arr.push("s")` on a `number[]` and `const s: string = arr.indexOf(1)` are still
// rejected, as tsc rejects them.
// Cost: callbacks are not checked against the element type, and what `map`, `reduce`, `find`
// and `flat` return is `any`, so what is done with it is unchecked; this loses errors and never
// invents one. Methods the library only adds for later targets (`at`, `flat`) are all offered.
fn array_member(arena: &mut TypeArena, element: TypeId, property_name: &str) -> Option<TypeId> {
    use crate::types::{FunctionType, Param};
    let any = arena.any();
    let number = arena.number();
    let string = arena.string();
    let boolean = arena.boolean();
    let void = arena.void();
    let undefined = arena.undefined();
    let elements = arena.alloc(Type::Array(element));
    let any_elements = arena.alloc(Type::Array(any));
    let param = |type_id: TypeId, optional: bool, rest: bool| Param {
        type_id,
        optional,
        rest,
        name: None,
    };
    let maybe_element = arena.alloc_union(vec![element, undefined]);
    let (params, return_type) = match property_name {
        "indexOf" | "lastIndexOf" => (
            vec![param(any, false, false), param(number, true, false)],
            number,
        ),
        "includes" => (
            vec![param(any, false, false), param(number, true, false)],
            boolean,
        ),
        "join" => (vec![param(string, true, false)], string),
        "push" | "unshift" => (vec![param(elements, false, true)], number),
        "pop" | "shift" => (Vec::new(), maybe_element),
        "at" => (vec![param(number, false, false)], maybe_element),
        "slice" => (
            vec![param(number, true, false), param(number, true, false)],
            elements,
        ),
        "splice" => (
            vec![
                param(number, false, false),
                param(number, true, false),
                param(any_elements, false, true),
            ],
            elements,
        ),
        "concat" => (vec![param(any_elements, false, true)], elements),
        "reverse" => (Vec::new(), elements),
        "sort" => (vec![param(any, true, false)], elements),
        "fill" => (
            vec![
                param(element, false, false),
                param(number, true, false),
                param(number, true, false),
            ],
            elements,
        ),
        "copyWithin" => (
            vec![
                param(number, false, false),
                param(number, false, false),
                param(number, true, false),
            ],
            elements,
        ),
        "filter" => (
            vec![param(any, false, false), param(any, true, false)],
            any_elements,
        ),
        "map" | "flatMap" => (
            vec![param(any, false, false), param(any, true, false)],
            any_elements,
        ),
        "flat" => (vec![param(number, true, false)], any_elements),
        "forEach" => (
            vec![param(any, false, false), param(any, true, false)],
            void,
        ),
        "some" | "every" => (
            vec![param(any, false, false), param(any, true, false)],
            boolean,
        ),
        "find" | "findLast" => (vec![param(any, false, false), param(any, true, false)], any),
        "findIndex" | "findLastIndex" => (
            vec![param(any, false, false), param(any, true, false)],
            number,
        ),
        "reduce" | "reduceRight" => (vec![param(any, false, false), param(any, true, false)], any),
        "keys" | "values" | "entries" => (Vec::new(), any),
        "toString" | "toLocaleString" => (Vec::new(), string),
        _ => return None,
    };
    Some(arena.alloc(Type::Function(FunctionType {
        params,
        return_type,
        type_params: Vec::new(),
        is_untyped: false,
    })))
}

// `then`, `catch` and `finally` of a Promise. Their real signatures chain a new Promise from
// what the callbacks return, which takes generic inference, so each takes callbacks and returns
// `any`: a chain after the first call is unchecked, and nothing it says is wrong.
fn promise_member(arena: &mut TypeArena, property_name: &str) -> Option<TypeId> {
    use crate::types::{FunctionType, Param};
    if !matches!(property_name, "then" | "catch" | "finally") {
        return None;
    }
    let any = arena.any();
    let callback = |optional: bool| Param {
        type_id: any,
        optional,
        rest: false,
        name: None,
    };
    let params = if property_name == "then" {
        vec![callback(true), callback(true)]
    } else {
        vec![callback(true)]
    };
    Some(arena.alloc(Type::Function(FunctionType {
        params,
        return_type: any,
        type_params: Vec::new(),
        is_untyped: false,
    })))
}

// Problem: `x.toString()` on a number, an enum object or a class reported "Property
// 'toString' does not exist", because a type only has the members written for it and no
// type carried the ones every value inherits from Object (and every function from Function).
// Picked: answer those few by name, loosely typed: arguments are `any` and the results are
// what the method returns, or `any` where that depends on the receiver.
// Cost: `(1).toString(1, 2)` and a wrong argument type are not caught, and a method that only
// strings or booleans have (`startsWith`, `padStart`) is still missing; numbers get toFixed,
// toExponential and toPrecision.
fn universal_member(
    arena: &mut TypeArena,
    receiver: TypeId,
    property_name: &str,
) -> Option<TypeId> {
    use crate::types::{FunctionType, Param};
    let any = arena.any();
    let any_array = arena.alloc(Type::Array(any));
    let rest = Param {
        type_id: any_array,
        optional: false,
        rest: true,
        name: None,
    };
    // The methods only a number has. Their one argument is an optional number, as in tsc.
    let on_number = matches!(arena.get(receiver), Type::Number | Type::NumberLiteral(_));
    if on_number && matches!(property_name, "toFixed" | "toExponential" | "toPrecision") {
        let digits = Param {
            type_id: arena.number(),
            optional: true,
            rest: false,
            name: None,
        };
        let string = arena.string();
        return Some(arena.alloc(Type::Function(FunctionType {
            params: vec![digits],
            return_type: string,
            type_params: Vec::new(),
            is_untyped: false,
        })));
    }
    let result = match property_name {
        "toString" | "toLocaleString" => arena.string(),
        "valueOf" => any,
        "hasOwnProperty" | "isPrototypeOf" | "propertyIsEnumerable" => arena.boolean(),
        "call" | "apply" | "bind" if matches!(arena.get(receiver), Type::Function(_)) => any,
        _ => return None,
    };
    Some(arena.alloc(Type::Function(FunctionType {
        params: vec![rest],
        return_type: result,
        type_params: Vec::new(),
        is_untyped: false,
    })))
}

// Problem: reading a property of `string | null` reported "Property 'length' does not
// exist on type 'string | null'", but tsc reports the cause, not the symptom:
// "'x' is possibly 'null'." (TS18047, 18048 for undefined, 18049 for both), and names the
// object the way it is written (`x`, `user.address`). Without a name (a call result, an
// index) tsc says "Object is possibly 'null'." (TS2531, 2532, 2533).
// Picked: report that once on the object, then look the property up on what is left
// once null and undefined are taken away, so a property that really is missing is still
// reported and one that is present is not. A type that is only null or undefined has
// nothing left, and tsc stops there, so the access is silently an error type.
//
// An optional chain is the one place that is not a mistake. `a?.b.c` is one chain: the
// undefined that `a?.b` gains from short-circuiting is not something `.c` has to
// guard against, and `a?.b` itself is the guard for `a`. Inside a chain the nullish
// members are taken away without a report, and the second value returned says so, so
// the caller can put `undefined` back on the result, as the chain would at runtime.
// Cost: inside a chain a member that is genuinely `T | null` is not reported either,
// since its type cannot be told apart from one the chain made nullish. That can only
// miss an error, never invent one.
pub(super) fn strip_nullish_object(
    object: &Expression,
    object_type: TypeId,
    optional: bool,
    ctx: &mut CheckContext<'_, '_>,
) -> (TypeId, bool) {
    let (has_null, has_undefined) = nullish_members(ctx, object_type);
    if !has_null && !has_undefined {
        return (object_type, false);
    }
    let rest = narrow_to_non_nullish(&mut ctx.arena, object_type);
    if optional {
        // `a?.b`: this link is the guard, and `with_optional` handles its own undefined.
        return (object_type, false);
    }
    if chain_has_optional(object) {
        return (rest, true);
    }
    ctx.error(
        crate::diagnostic_messages::messages::possibly_nullish(
            entity_name(object).as_deref(),
            has_null,
            has_undefined,
        ),
        object.span(),
    );
    if rest == ctx.arena.never() {
        return (ctx.arena.error(), false);
    }
    (rest, false)
}

// The result of a link inside an optional chain gains `undefined`: if an earlier `?.`
// short-circuited, the whole chain is undefined.
pub(super) fn with_chain_undefined(
    result: TypeId,
    short_circuits: bool,
    ctx: &mut CheckContext<'_, '_>,
) -> TypeId {
    if !short_circuits {
        return result;
    }
    let undefined = ctx.arena.undefined();
    ctx.arena.alloc_union(vec![result, undefined])
}

// True when `expr` is, or hangs off, an optional link (`a?.b`, `a?.[0]`, `a?.()`).
// Parentheses end the chain, as they do in tsc: `(a?.b).c` is checked like any access.
fn chain_has_optional(expr: &Expression) -> bool {
    match expr {
        Expression::StaticMemberExpression(member) => {
            member.optional || chain_has_optional(&member.object)
        }
        Expression::ComputedMemberExpression(member) => {
            member.optional || chain_has_optional(&member.object)
        }
        Expression::CallExpression(call) => call.optional || chain_has_optional(&call.callee),
        _ => false,
    }
}

pub(super) fn infer_member_access_type_with_optional(
    object_type: TypeId,
    property_name: &str,
    optional: bool,
    span: Span,
    ctx: &mut CheckContext<'_, '_>,
) -> TypeId {
    if !optional {
        return infer_member_access_type(object_type, property_name, span, ctx);
    }

    // a?.b short-circuits to undefined at runtime when a is null or undefined, so
    // the property is looked up on the non-nullish narrowing of the object type,
    // and undefined is added back into the result to reflect that short-circuit.
    let non_nullish = narrow_to_non_nullish(&mut ctx.arena, object_type);
    let property_type = infer_member_access_type(non_nullish, property_name, span, ctx);
    ctx.arena
        .alloc_union(vec![property_type, ctx.arena.undefined()])
}

// `length` of a tuple: the one length a fixed tuple has, the lengths between the required
// count and the full count when elements are optional, and plain `number` with a rest element.
fn tuple_length_type(arena: &mut TypeArena, elements: &[crate::types::TupleElement]) -> TypeId {
    if elements.iter().any(|e| e.rest) {
        return arena.number();
    }
    let required = elements.iter().filter(|e| !e.optional).count();
    let lengths: Vec<TypeId> = (required..=elements.len())
        .map(|length| arena.alloc(Type::NumberLiteral(length as f64)))
        .collect();
    arena.alloc_union(lengths)
}

// What any element of the tuple can be, the type an array of them would have. An optional
// element can be missing, so `with_missing` adds `undefined` for it (an index read does,
// a method receiver does not). The empty tuple has no element: `never`.
pub(crate) fn tuple_element_union(
    arena: &mut TypeArena,
    elements: &[crate::types::TupleElement],
    with_missing: bool,
) -> TypeId {
    let mut members = Vec::with_capacity(elements.len() + 1);
    for element in elements {
        if !members.contains(&element.type_id) {
            members.push(element.type_id);
        }
        if with_missing && element.optional {
            let undefined = arena.undefined();
            if !members.contains(&undefined) {
                members.push(undefined);
            }
        }
    }
    match members.len() {
        0 => arena.never(),
        1 => members[0],
        _ => arena.alloc_union(members),
    }
}

pub(super) fn infer_computed_member_access_type(
    object_type: TypeId,
    key_expr: &Expression,
    optional: bool,
    span: Span,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> TypeId {
    let key_type = infer_expression_type(key_expr, scoping, ctx);

    // A Record<K, V>-tagged object accepts any key and yields V; see
    // TypeArena::record_value_type's own doc comment on why K itself is not
    // checked here.
    if let Some(value_type) = ctx.arena.record_value_type(object_type) {
        return value_type;
    }

    // A tuple knows its elements, so a literal index names one of them: `t[0]` on
    // `[string, number]` is `string`, with no `undefined` added, and an index past the end
    // is an error, as in tsc. Any other number-typed index can be any element.
    if let Type::Tuple(elements) = ctx.arena.get(object_type).clone() {
        let rest = elements.last().filter(|e| e.rest).map(|e| e.type_id);
        let fixed = elements.len() - usize::from(rest.is_some());
        match ctx.arena.get(key_type) {
            &Type::NumberLiteral(value) if value >= 0.0 && value.fract() == 0.0 => {
                let index = value as usize;
                if index < fixed {
                    let element = elements[index];
                    return if element.optional {
                        ctx.arena
                            .alloc_union(vec![element.type_id, ctx.arena.undefined()])
                    } else {
                        element.type_id
                    };
                }
                return match rest {
                    // Past the fixed elements a rest element answers, as an array does.
                    Some(rest) => ctx.arena.alloc_union(vec![rest, ctx.arena.undefined()]),
                    // tsc reports the index (TS2493 at the number, not at the whole access)
                    // and then reads it as `undefined`, so `const n: number = t[2]` is a
                    // second error there too.
                    None => {
                        ctx.error(
                            crate::diagnostic_messages::messages::tuple_index_out_of_bounds(
                                &ctx.arena,
                                object_type,
                                elements.len(),
                                index,
                            ),
                            key_expr.span(),
                        );
                        ctx.arena.undefined()
                    }
                };
            }
            // `t[-1]`: TS2514 at the index, and the read is `undefined`. A tuple with a rest
            // element is left to the paths below, as tsc does.
            &Type::NumberLiteral(value)
                if value < 0.0 && value.fract() == 0.0 && rest.is_none() =>
            {
                ctx.error(
                    crate::diagnostic_messages::messages::tuple_negative_index(),
                    key_expr.span(),
                );
                return ctx.arena.undefined();
            }
            Type::Number | Type::Any => {
                let union = tuple_element_union(&mut ctx.arena, &elements, true);
                return if rest.is_some() {
                    ctx.arena.alloc_union(vec![union, ctx.arena.undefined()])
                } else {
                    union
                };
            }
            _ => {}
        }
    }

    if let &Type::Array(element_type) = ctx.arena.get(object_type) {
        // Every access to an array element, whether the index is a literal or
        // not, includes undefined: a literal index being in range is no more
        // provable at compile time than a variable one is, since this checker
        // (like tsc under noUncheckedIndexedAccess) does not track array
        // lengths. `arr[0]` and `arr[i]` are both indexing past the end of a
        // real array at runtime if the array turns out to be empty.
        if !ctx.options.no_unchecked_indexed_access {
            return element_type;
        }
        return ctx
            .arena
            .alloc_union(vec![element_type, ctx.arena.undefined()]);
    }

    let Expression::StringLiteral(key) = key_expr else {
        // A key typed plain `string`, `number` or `any` cannot name any particular
        // property, and a plain object type has no index signature to fall back
        // on, so tsc rejects the access under noImplicitAny. Any other key (a
        // literal type, a union of literals, a generic parameter) may well be
        // valid, and is a shape this checker cannot evaluate yet, so it is still
        // reported as unsupported rather than guessed at.
        let plain_object = matches!(ctx.arena.get(object_type), Type::Object(_));
        let plain_key = matches!(
            ctx.arena.get(key_type),
            Type::String | Type::Number | Type::Any
        );
        if plain_object && plain_key {
            ctx.error(
                crate::diagnostic_messages::messages::element_implicitly_any(
                    &ctx.arena,
                    key_type,
                    object_type,
                ),
                span,
            );
        } else {
            ctx.warning(
                crate::diagnostic_messages::messages::unimplemented_computed_member_key(),
                span,
            );
        }
        return ctx.arena.error();
    };

    infer_member_access_type_with_optional(object_type, &key.value, optional, span, ctx)
}

pub(super) fn infer_chain_element_type(
    element: &oxc_ast::ast::ChainElement,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> TypeId {
    use oxc_ast::ast::ChainElement;

    match element {
        // The last link of a chain. `a?.b.c` ends here, on `.c`, whose object `a?.b` is
        // `Inner | undefined`: the undefined is the chain's short-circuit, so it is taken
        // away for the lookup and put back on the result (see strip_nullish_object).
        ChainElement::StaticMemberExpression(member) => {
            let object_type = infer_expression_type(&member.object, scoping, ctx);
            let (object_type, short_circuits) =
                strip_nullish_object(&member.object, object_type, member.optional, ctx);
            let property_type = infer_member_access_type_with_optional(
                object_type,
                &member.property.name,
                member.optional,
                member.span(),
                ctx,
            );
            with_chain_undefined(property_type, short_circuits, ctx)
        }
        ChainElement::ComputedMemberExpression(member) => {
            let object_type = infer_expression_type(&member.object, scoping, ctx);
            let (object_type, short_circuits) =
                strip_nullish_object(&member.object, object_type, member.optional, ctx);
            let element_type = infer_computed_member_access_type(
                object_type,
                &member.expression,
                member.optional,
                member.span(),
                scoping,
                ctx,
            );
            with_chain_undefined(element_type, short_circuits, ctx)
        }
        _ => {
            ctx.warning(
                crate::diagnostic_messages::messages::unimplemented_optional_chain_link(),
                element.span(),
            );
            ctx.arena.error()
        }
    }
}
