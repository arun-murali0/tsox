use oxc_ast::ast::Expression;
use oxc_semantic::Scoping;
use oxc_span::{GetSpan, Span};

use crate::arena::TypeId;
use crate::semantic::{GenericInferenceSession, GenericSolver};
use crate::types::{FunctionType, Type};

use super::super::context::CheckContext;
use super::super::narrow::resolve_symbol_id;
use super::infer_expression_type;
use super::{check_excess_properties, infer_member_access_type, resolve_identifier_type};

// Resolves an explicit call-site type argument list, e.g. the <string> in
// identity<string>(x), into concrete TypeIds. Returns an empty vec for a call
// with no such list (`call.type_arguments`/`new_expr.type_arguments` being the
// ordinary, common case: a bare `identity(x)`), so callers don't need to
// special-case "none given" separately from "given but unresolvable".
// An individual type argument this checker cannot resolve (it names something not in
// scope) becomes the error type instead of being dropped. The arguments are matched to
// the type parameters by position, so dropping one would shift every later argument onto
// the wrong parameter: `two<Nope, string>(1, "x")` would bind `string` to the first
// parameter and report a bogus mismatch on the first argument. The error type is
// compatible with everything, so the parameter it lands on stops constraining its
// arguments without causing a second diagnostic.
fn resolve_explicit_type_arguments(
    type_arguments: Option<&oxc_ast::ast::TSTypeParameterInstantiation>,
    ctx: &mut CheckContext<'_, '_>,
) -> Vec<TypeId> {
    let Some(type_arguments) = type_arguments else {
        return Vec::new();
    };
    let mut resolved = Vec::with_capacity(type_arguments.params.len());
    for ty in &type_arguments.params {
        let type_id =
            crate::type_annotation::resolve_ts_type(ty, &mut ctx.namespace, &mut ctx.arena)
                .unwrap_or_else(|| ctx.arena.error());
        resolved.push(type_id);
    }
    resolved
}

// Everything about one call or `new` site that check_callable needs besides the
// callee's type, the scope info, and the checking context. Grouped into one value
// so check_callable stays under clippy's too_many_arguments limit instead of
// growing another positional parameter every time a call feature is added.
struct CallSite<'s, 'ast> {
    arguments: &'s [oxc_ast::ast::Argument<'ast>],
    span: Span,
    callee_name: &'s str,
    is_new: bool,
    // True when the callee is written as the name of a function or class declaration.
    // Such a call instantiates the declaration afresh, even from inside its own body,
    // so every type parameter in its type is the callee's own to infer. For any other
    // callee (a parameter, a variable, a method) a parameter of an enclosing
    // declaration that appears in its type is not.
    callee_is_declaration: bool,
    explicit_type_args: &'s [TypeId],
}

// Only a plain name or an `a.b` member is a callee this checker can type. Any other callee
// (the result of another call, a computed access) gets the not-yet-checked warning and the
// error type, which is compatible with everything, so a call it cannot read adds no
// second error. A callee written as a function declaration's name is marked, so the call
// instantiates the declaration afresh even from inside its own body (see check_callable).
pub(super) fn infer_call_expression_type(
    call: &oxc_ast::ast::CallExpression,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> TypeId {
    let mut callee_is_declaration = false;
    let (callee_type, callee_name) = match &call.callee {
        Expression::Identifier(ident) => {
            let callee_symbol = resolve_symbol_id(ident, scoping)
                .filter(|symbol_id| scoping.symbol_flags(*symbol_id).is_function());
            callee_is_declaration = callee_symbol.is_some();
            if let Some(symbol_id) = callee_symbol {
                ctx.return_reads.note_call(symbol_id);
            }
            (
                resolve_identifier_type(ident, scoping, ctx),
                ident.name.to_string(),
            )
        }
        Expression::StaticMemberExpression(member) => member_callee(member, scoping, ctx),
        _ => {
            ctx.warning(
                crate::diagnostic_messages::messages::unimplemented_call_expression_kind(),
                call.span(),
            );
            return ctx.arena.error();
        }
    };

    let explicit_type_args = resolve_explicit_type_arguments(call.type_arguments.as_deref(), ctx);

    check_callable(
        callee_type,
        CallSite {
            arguments: &call.arguments,
            span: call.span(),
            callee_name: &callee_name,
            is_new: false,
            callee_is_declaration,
            explicit_type_args: &explicit_type_args,
        },
        scoping,
        ctx,
    )
}

// The type and the name of an `a.b` callee: the member's type read off the object's, with
// a nullish object reported once, as any other member access is.
fn member_callee(
    member: &oxc_ast::ast::StaticMemberExpression,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> (TypeId, String) {
    let object_type = infer_expression_type(&member.object, scoping, ctx);
    let (object_type, _) =
        super::members::strip_nullish_object(&member.object, object_type, member.optional, ctx);
    let property_type =
        infer_member_access_type(object_type, &member.property.name, member.span(), ctx);
    (property_type, member.property.name.to_string())
}

// `new Name(...)` and `new a.b(...)` (`new N.K()`, `new A.B.K()`). Either names a class
// declaration, so the callee is marked as one like a function's name is: `new Box(x)`
// inside `Box` infers `T` again instead of reusing the one being checked, and
// `new N.Box(x)` is read the same way. Any other callee (a call's result, a computed
// access, a parenthesized expression) gets the not-yet-checked warning and the error type,
// which is compatible with everything, so a `new` this checker cannot read adds no second
// error.
//
// Problem: only `new Name(...)` was read, so `new N.K()` on a class inside a namespace was
// skipped with a warning, and its result was the error type, which hid every mistake made
// with the instance and every wrong argument to its constructor.
// Picked: take the member's type the way a call on `a.b` does (member_callee) and run the
// same check as for a plain name. The namespace's value already holds the class's
// constructor as a property, so there is nothing new to look up.
pub(super) fn infer_new_expression_type(
    new_expr: &oxc_ast::ast::NewExpression,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> TypeId {
    let (callee_type, callee_name) = match &new_expr.callee {
        Expression::Identifier(callee_ident) => (
            resolve_identifier_type(callee_ident, scoping, ctx),
            callee_ident.name.to_string(),
        ),
        Expression::StaticMemberExpression(member) => member_callee(member, scoping, ctx),
        _ => {
            ctx.warning(
                crate::diagnostic_messages::messages::unimplemented_new_expression_target(),
                new_expr.span(),
            );
            return ctx.arena.error();
        }
    };
    let explicit_type_args =
        resolve_explicit_type_arguments(new_expr.type_arguments.as_deref(), ctx);

    check_callable(
        callee_type,
        CallSite {
            arguments: &new_expr.arguments,
            span: new_expr.span(),
            callee_name: &callee_name,
            is_new: true,
            callee_is_declaration: true,
            explicit_type_args: &explicit_type_args,
        },
        scoping,
        ctx,
    )
}

enum PickedCallable {
    // The member to check the call against in the ordinary way.
    Member(TypeId),
    // The call has been reported already; this is the type it is left with.
    Reported(TypeId),
}

// How one function signature meets a call's arguments.
enum Fit {
    Fits,
    WrongArity,
    // The first argument that is not assignable to its parameter.
    WrongArgument { index: usize, parameter: TypeId },
}

// A call through an intersection of functions uses the first member that accepts it, in the
// order the members were written (LLD 1.13): `F1 & F2` called with an argument both accept
// runs `F1`, and `F2 & F1` runs `F2`. There are no overloads elsewhere in this checker, so
// this is the one place a callee has more than one signature.
//
// When no member accepts the call, tsc does not simply blame the first one:
// - if every member fails on how many arguments there are, it is one TS2554 for the whole
//   set, with the range the members allow together (`Expected 1-2 arguments`);
// - if just one member gets past the arity check, that member's own error is the answer
//   (the ordinary TS2345), so it is handed back to be checked like any single signature;
// - if two or more do, it is TS2769 "No overload matches this call." with one branch per
//   such overload saying why it was rejected, numbered among those overloads. It sits on
//   the argument when every overload fails on the same one and on the callee otherwise.
// The callee's position is the call's span here; the comparison with tsc goes by line.
//
// The argument types are inferred once here and whatever that reported is dropped; the
// ordinary check of the member picked then infers and reports them again, so each problem
// in an argument is still reported once. A spread argument is not probed: its length
// is not known, so the first function member is used.
fn pick_callable_member(
    callee_type: TypeId,
    arguments: &[oxc_ast::ast::Argument<'_>],
    call_span: Span,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> PickedCallable {
    let Type::Intersection(members) = ctx.arena.get(callee_type).clone() else {
        return PickedCallable::Member(callee_type);
    };
    if ctx.arena.intersection_reduces_to_never(callee_type) {
        return PickedCallable::Member(callee_type);
    }
    let callable: Vec<TypeId> = members
        .into_iter()
        .filter(|&member| matches!(ctx.arena.get(member), Type::Function(_)))
        .collect();
    match callable.as_slice() {
        [] => PickedCallable::Member(callee_type),
        [only] => PickedCallable::Member(*only),
        _ => {
            let has_spread = arguments
                .iter()
                .any(|argument| matches!(argument, oxc_ast::ast::Argument::SpreadElement(_)));
            if has_spread {
                return PickedCallable::Member(callable[0]);
            }
            let reported_before = ctx.diagnostics.len();
            let argument_types: Vec<TypeId> = arguments
                .iter()
                .filter_map(|argument| argument.as_expression())
                .map(|expression| infer_expression_type(expression, scoping, ctx))
                .collect();
            ctx.diagnostics.truncate(reported_before);

            let fits: Vec<(TypeId, Fit)> = callable
                .iter()
                .map(|&member| {
                    let fit = match ctx.arena.get(member) {
                        Type::Function(function) => fit_of(&ctx.arena, function, &argument_types),
                        _ => Fit::WrongArity,
                    };
                    (member, fit)
                })
                .collect();
            if let Some((member, _)) = fits.iter().find(|(_, fit)| matches!(fit, Fit::Fits)) {
                return PickedCallable::Member(*member);
            }

            let rejected: Vec<(TypeId, usize, TypeId)> = fits
                .iter()
                .filter_map(|(member, fit)| match fit {
                    Fit::WrongArgument { index, parameter } => Some((*member, *index, *parameter)),
                    _ => None,
                })
                .collect();
            match rejected.as_slice() {
                [] => report_arity_over_all(&callable, arguments, call_span, ctx),
                [(only, ..)] => PickedCallable::Member(*only),
                _ => report_no_overload_matches(
                    &rejected,
                    &argument_types,
                    arguments,
                    call_span,
                    ctx,
                ),
            }
        }
    }
}

// Every member fails on the number of arguments: one TS2554 over the range the members allow
// together, from the fewest any of them needs to the most any of them takes (open-ended when
// one has a rest parameter).
fn report_arity_over_all(
    callable: &[TypeId],
    arguments: &[oxc_ast::ast::Argument<'_>],
    call_span: Span,
    ctx: &mut CheckContext<'_, '_>,
) -> PickedCallable {
    let mut fewest = usize::MAX;
    let mut most: Option<usize> = Some(0);
    for &member in callable {
        if let Type::Function(function) = ctx.arena.get(member) {
            let expanded = expand_tuple_rest(&ctx.arena, function);
            let function = expanded.as_ref().unwrap_or(function);
            let required = function
                .params
                .iter()
                .filter(|param| !param.optional && !param.rest)
                .count();
            fewest = fewest.min(required);
            let has_rest = function.params.last().is_some_and(|param| param.rest);
            most = match (most, has_rest) {
                (_, true) | (None, _) => None,
                (Some(current), false) => Some(current.max(function.params.len())),
            };
        }
    }
    let arity_span = match most {
        Some(most) if arguments.len() > most => arguments[most].span(),
        _ => call_span,
    };
    ctx.error(arity_message(fewest, most, arguments.len()), arity_span);
    let first_return = match ctx.arena.get(callable[0]) {
        Type::Function(function) => function.return_type,
        _ => ctx.arena.error(),
    };
    PickedCallable::Reported(first_return)
}

// Two or more members passed the arity check and each failed on an argument: tsc's TS2769.
// Each branch is the line naming the overload and the reason under it, which is the
// ordinary "Argument of type ... is not assignable to parameter of type ..." text.
fn report_no_overload_matches(
    rejected: &[(TypeId, usize, TypeId)],
    argument_types: &[TypeId],
    arguments: &[oxc_ast::ast::Argument<'_>],
    call_span: Span,
    ctx: &mut CheckContext<'_, '_>,
) -> PickedCallable {
    let mut text = String::from("No overload matches this call.");
    for (number, (member, index, parameter)) in rejected.iter().enumerate() {
        let signature = match ctx.arena.get(*member) {
            Type::Function(function) => {
                crate::type_display::display_signature(&ctx.arena, function)
            }
            _ => String::new(),
        };
        text.push_str(&format!(
            "\n  Overload {} of {}, '{}', gave the following error.",
            number + 1,
            rejected.len(),
            signature
        ));
        let reason = crate::diagnostic_messages::messages::argument_not_assignable(
            &ctx.arena,
            argument_types[*index],
            *parameter,
        );
        for line in reason.text.lines() {
            text.push_str("\n    ");
            text.push_str(line);
        }
    }

    // Overloads that all failed on the same argument blame that argument, so the error
    // sits on it. Failing on different ones, no single argument is to blame, so it sits on
    // the call.
    let same_argument = rejected.iter().all(|(_, index, _)| *index == rejected[0].1);
    let span = if same_argument {
        arguments[rejected[0].1].span()
    } else {
        call_span
    };
    ctx.error(
        crate::diagnostic_messages::messages::no_overload_matches(text),
        span,
    );
    let first_return = match ctx.arena.get(rejected[0].0) {
        Type::Function(function) => function.return_type,
        _ => ctx.arena.error(),
    };
    PickedCallable::Reported(first_return)
}

// `(...args: [string, number?])` takes the elements of its tuple as separate arguments, so
// it is called like `(a: string, b?: number)`: how many arguments it needs, what each one
// must be and which are optional all come from the tuple. A rest parameter that is not a
// tuple (an array, or a type parameter) is left as it is. The rest element of such a tuple
// becomes a rest parameter that carries its element type directly.
fn expand_tuple_rest(
    arena: &crate::arena::TypeArena,
    function: &FunctionType,
) -> Option<FunctionType> {
    let last = function.params.last().filter(|p| p.rest)?;
    let Type::Tuple(elements) = arena.get(last.type_id) else {
        return None;
    };
    let mut expanded = function.clone();
    expanded.params.pop();
    for element in elements {
        expanded.params.push(crate::types::Param {
            type_id: element.type_id,
            optional: element.optional || element.rest,
            rest: element.rest,
            name: None,
        });
    }
    Some(expanded)
}

// How a call with these argument types meets the function: enough arguments, not too
// many, and each one assignable to its parameter. A parameter that mentions a type
// parameter is not judged here, since what it accepts depends on inference; arity decides.
fn fit_of(
    arena: &crate::arena::TypeArena,
    function: &FunctionType,
    argument_types: &[TypeId],
) -> Fit {
    let expanded = expand_tuple_rest(arena, function);
    let function = expanded.as_ref().unwrap_or(function);
    if function.is_untyped {
        return Fit::Fits;
    }
    let required = function
        .params
        .iter()
        .filter(|param| !param.optional && !param.rest)
        .count();
    let rest = function.params.last().filter(|param| param.rest);
    if argument_types.len() < required {
        return Fit::WrongArity;
    }
    if rest.is_none() && argument_types.len() > function.params.len() {
        return Fit::WrongArity;
    }

    for (index, &argument) in argument_types.iter().enumerate() {
        let parameter = match function.params.get(index) {
            Some(param) if !param.rest => param.type_id,
            _ => match rest {
                Some(rest) => match arena.get(rest.type_id) {
                    Type::Array(element) => *element,
                    _ => rest.type_id,
                },
                None => continue,
            },
        };
        let judged = GenericSolver::contains_type_param(arena, parameter)
            || crate::subtyping::is_subtype(arena, argument, parameter);
        if !judged {
            return Fit::WrongArgument { index, parameter };
        }
    }
    Fit::Fits
}

/// Whether an expression holds a function whose parameters need a contextual type, the way
/// tsc defines a context-sensitive argument: a function or arrow with a parameter that has
/// no annotation, found directly or inside an object or array literal. Only such arguments
/// are put off until the other arguments have bound the type parameters; an object whose
/// callbacks are all annotated is inferred in the first pass like any ordinary argument.
fn is_context_sensitive(expr: &Expression) -> bool {
    use oxc_ast::ast::{ArrayExpressionElement, ObjectPropertyKind};
    match expr {
        Expression::ArrowFunctionExpression(arrow) => arrow
            .params
            .items
            .iter()
            .any(|param| param.type_annotation.is_none()),
        Expression::FunctionExpression(function) => function
            .params
            .items
            .iter()
            .any(|param| param.type_annotation.is_none()),
        Expression::ParenthesizedExpression(inner) => is_context_sensitive(&inner.expression),
        Expression::ObjectExpression(object) => {
            object.properties.iter().any(|property| match property {
                ObjectPropertyKind::ObjectProperty(property) => {
                    is_context_sensitive(&property.value)
                }
                ObjectPropertyKind::SpreadProperty(_) => false,
            })
        }
        Expression::ArrayExpression(array) => array.elements.iter().any(|element| match element {
            ArrayExpressionElement::SpreadElement(_) | ArrayExpressionElement::Elision(_) => false,
            other => other.as_expression().is_some_and(is_context_sensitive),
        }),
        _ => false,
    }
}

fn check_callable(
    callee_type: TypeId,
    site: CallSite<'_, '_>,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> TypeId {
    let CallSite {
        arguments,
        span,
        callee_name,
        is_new,
        callee_is_declaration,
        explicit_type_args,
    } = site;

    let callee_type = match pick_callable_member(callee_type, arguments, span, scoping, ctx) {
        PickedCallable::Member(member) => member,
        // The call was reported as a whole (tsc's TS2769, or one arity message over every
        // overload). The arguments are still looked at for their own mistakes, once.
        PickedCallable::Reported(return_type) => {
            for arg in arguments {
                if let Some(arg_expr) = arg.as_expression() {
                    infer_expression_type(arg_expr, scoping, ctx);
                }
            }
            return return_type;
        }
    };

    let Type::Function(function_type) = ctx.arena.get(callee_type).clone() else {
        // Any and Error both mean "do not report a second, likely-noisy error on
        // top of one already reported (or deliberately suppressed) elsewhere."
        // Arguments are still checked for their own independent problems even
        // though the call itself is not.
        if !matches!(ctx.arena.get(callee_type), Type::Any | Type::Error) {
            let message = if is_new {
                crate::diagnostic_messages::messages::not_a_constructor(callee_name)
            } else {
                crate::diagnostic_messages::messages::not_callable(callee_name)
            };
            ctx.error(message, span);
            return ctx.arena.error();
        }

        for arg in arguments {
            if let Some(arg_expr) = arg.as_expression() {
                infer_expression_type(arg_expr, scoping, ctx);
            }
        }
        return ctx.arena.error();
    };

    // A parameter this checker could not resolve a type for (declare_top_level
    // left it untyped) means arity cannot be verified honestly, so it is skipped
    // with a warning rather than silently allowed or wrongly flagged.
    if function_type.is_untyped {
        let message = if is_new {
            crate::diagnostic_messages::messages::untyped_constructor_parameter_skips_arity_check(
                callee_name,
            )
        } else {
            crate::diagnostic_messages::messages::untyped_parameter_skips_arity_check(callee_name)
        };
        ctx.warning(message, span);
        for arg in arguments {
            if let Some(arg_expr) = arg.as_expression() {
                infer_expression_type(arg_expr, scoping, ctx);
            }
        }
        return function_type.return_type;
    }

    // Calls may omit trailing type parameters only when those parameters have defaults.
    // Supplying a partial list for required parameters, or more arguments than exist, is
    // TS2558. Like tsc, the call is then abandoned: the arguments are still walked so
    // errors inside them show, but they are not matched against the parameters, and the
    // result is the error type so nothing cascades from it.
    if !explicit_type_args.is_empty() {
        let required_type_args = function_type
            .type_params
            .iter()
            .rposition(|parameter| parameter.default.is_none())
            .map_or(0, |index| index + 1);
        let total_type_args = function_type.type_params.len();
        if explicit_type_args.len() < required_type_args
            || explicit_type_args.len() > total_type_args
        {
            ctx.error(
                crate::diagnostic_messages::messages::call_type_argument_count_mismatch(
                    required_type_args,
                    total_type_args,
                    explicit_type_args.len(),
                ),
                span,
            );
            for arg in arguments {
                if let Some(arg_expr) = arg.as_expression() {
                    infer_expression_type(arg_expr, scoping, ctx);
                }
            }
            return ctx.arena.error();
        }
    }

    let function_type = expand_tuple_rest(&ctx.arena, &function_type).unwrap_or(function_type);

    let required = function_type
        .params
        .iter()
        .filter(|p| !p.optional && !p.rest)
        .count();
    let has_rest = function_type.params.last().is_some_and(|p| p.rest);
    let max = if has_rest {
        None
    } else {
        Some(function_type.params.len())
    };

    // Problem: `f(...xs)` counted the spread as one argument, so a spread that
    // stands for several (or none) tripped a false arity error.
    // Picked: up to the first spread the arguments are positional and can be
    // counted; the spread's own length is not known, so only "too many" can be
    // decided, from the arguments written before it.
    // Cost: a call like `f(...xs)` with too few values is not reported.
    //
    // Now: when the first spread is known to be an array (a tuple is not one here: it keeps
    // the lenient rule, since its elements are not matched to parameters), tsc's own rule applies:
    // the spread is only accepted when it starts at or after the required parameters
    // and lands on a rest parameter or a parameter that exists. Anything else is TS2556 on
    // the spread, which tsc reports instead of an arity message. A spread of anything that
    // is not known to be an array (any, an unresolved type) keeps the lenient rule above,
    // so this adds no false errors.
    let first_spread = arguments
        .iter()
        .position(|arg| matches!(arg, oxc_ast::ast::Argument::SpreadElement(_)));
    // The first spread is inferred here, once, because the check below needs its type;
    // the pass over the arguments further down skips it so it is not reported twice.
    let first_spread_type = first_spread.and_then(|index| match &arguments[index] {
        oxc_ast::ast::Argument::SpreadElement(spread) => {
            Some(infer_expression_type(&spread.argument, scoping, ctx))
        }
        _ => None,
    });
    let misplaced_spread = first_spread.zip(first_spread_type).and_then(|(index, ty)| {
        // `f(...[1, 2])` is a tuple to tsc (the literal is typed by where it lands), so
        // spreading an array literal is never an error here.
        let is_literal = matches!(
            &arguments[index],
            oxc_ast::ast::Argument::SpreadElement(spread) if is_array_literal(&spread.argument)
        );
        let is_array = matches!(ctx.arena.get(ty), Type::Array(_));
        let accepted = index >= required && (has_rest || index < function_type.params.len());
        (is_array && !is_literal && !accepted).then_some(index)
    });
    let arity_ok = misplaced_spread.is_none()
        && match first_spread {
            Some(first) => match max {
                Some(max) => first <= max,
                None => true,
            },
            None => {
                arguments.len() >= required
                    && match max {
                        Some(max) => arguments.len() <= max,
                        None => true,
                    }
            }
        };
    if !arity_ok {
        if let Some(index) = misplaced_spread {
            ctx.error(
                crate::diagnostic_messages::messages::spread_argument_needs_tuple_or_rest(),
                arguments[index].span(),
            );
        } else {
            // The text is tsc's TS2554 verbatim, which never names the missing
            // parameter, so there is nothing to look up here.
            // tsc puts "too many arguments" on the first extra argument and "too few" on
            // the call, so each lands on the line a reader would look at.
            let arity_span = match max {
                Some(max) if arguments.len() > max => arguments[max].span(),
                _ => span,
            };
            ctx.error(arity_message(required, max, arguments.len()), arity_span);
        }

        for arg in arguments {
            if let Some(arg_expr) = arg.as_expression() {
                infer_expression_type(arg_expr, scoping, ctx);
            }
        }
        return function_type.return_type;
    }

    // Every argument's type is inferred once up front and reused below, rather
    // than inferred again inside the parameter-checking loop, since inferring an
    // argument's type can itself report diagnostics; inferring it twice would
    // report the same problem in that argument twice.
    //
    // Now: a spread argument is inferred for its own errors but gets no type, and so
    // does every argument after it, because once a spread has been seen the index of
    // a later argument no longer lines up with a parameter position. A missing type
    // is skipped by both the inference pass and the assignability pass below.
    // Infer ordinary arguments first. Direct callbacks are delayed until we have a
    // preliminary view of the generic bindings, so their unannotated parameters can
    // be contextually typed with concrete types instead of the caller's raw `T`/`U`.
    // Each expression is still inferred exactly once, preserving diagnostic behavior.
    let mut arg_types: Vec<Option<TypeId>> = vec![None; arguments.len()];
    let mut deferred_contextual_args = Vec::new();
    // An object literal can contain callbacks several properties below the argument
    // itself. Delay such literals when another ordinary argument can establish generic
    // bindings first. Object-only calls are left on the ordinary path so their literal
    // properties remain available as the primary inference candidates.
    let has_non_callback_non_object_anchor = arguments.iter().any(|argument| {
        argument.as_expression().is_some_and(|expression| {
            !matches!(
                expression,
                Expression::ArrowFunctionExpression(_)
                    | Expression::FunctionExpression(_)
                    | Expression::ObjectExpression(_)
            )
        })
    });
    for (index, arg) in arguments.iter().enumerate() {
        let after_spread = first_spread.is_some_and(|first| index >= first);
        match arg {
            oxc_ast::ast::Argument::SpreadElement(spread) => {
                if Some(index) != first_spread {
                    infer_expression_type(&spread.argument, scoping, ctx);
                }
            }
            other => {
                let Some(expr) = other.as_expression() else {
                    continue;
                };
                let is_callback = matches!(
                    expr,
                    Expression::ArrowFunctionExpression(_) | Expression::FunctionExpression(_)
                );
                let is_contextual_object = has_non_callback_non_object_anchor
                    && matches!(expr, Expression::ObjectExpression(_))
                    && is_context_sensitive(expr);
                if (is_callback || is_contextual_object) && !after_spread {
                    deferred_contextual_args.push(index);
                    continue;
                }
                ctx.contextual = if after_spread {
                    None
                } else {
                    GenericSolver::expected_param_type(&ctx.arena, &function_type.params, index)
                };
                let inferred = infer_expression_type(expr, scoping, ctx);
                if !after_spread {
                    arg_types[index] = Some(inferred);
                }
            }
        })
        .collect();

    // A first pass over every argument collects generic parameter bindings before
    // any argument is checked against its expected type, so a generic function's
    // return type can be substituted correctly even when the argument that fixes
    // a type parameter comes after other checked arguments.
    // An explicit call-site type argument list (identity<string>(x)) is
    // positional, with no declaration span of its own, so it is zipped against
    // the function's own type parameters in the order they were declared;
    // see ordered_generic_param_ids for how that order is recovered. Extra
    // type arguments beyond the function's own parameter count are ignored,
    // and a partial list (some but not all of the function's type parameters
    // given explicitly) leaves the rest to ordinary argument-driven inference,
    // matching this checker's general "leave the rest to be inferred/left
    // unresolved" stance rather than treating it as an arity error.
    let mut declared_param_ids = Vec::new();
    // Signature metadata preserves declaration order and parameters that do not occur
    // structurally (for example `<T = string>() => T` or `<T>() => void`).
    for type_param in &function_type.type_params {
        if !declared_param_ids.contains(&type_param.id) {
            declared_param_ids.push(type_param.id);
        }
    }
    // Retain the graph walk for enclosing generic parameters, e.g. a method using the
    // containing class's T. The walker intentionally does not pull in a nested callback's
    // own type-parameter list as if it belonged to this call.
    for param in &function_type.params {
        GenericSolver::ordered_generic_param_ids(
            &ctx.arena,
            param.type_id,
            &mut declared_param_ids,
        );
    }
    GenericSolver::ordered_generic_param_ids(
        &ctx.arena,
        function_type.return_type,
        &mut declared_param_ids,
    );

    // A parameter of a declaration the checker is currently inside is fixed, not
    // inferred: its binding is itself, so substitution leaves it alone. Explicit type
    // arguments are zipped against the remaining parameters, the callee's own.
    let ambient_ids: Vec<crate::types::TypeParameterId> = if callee_is_declaration {
        Vec::new()
    } else {
        declared_param_ids
            .iter()
            .copied()
            .filter(|&id| ctx.namespace.is_type_param_in_scope(id))
            .collect()
    };
    let own_ids: Vec<crate::types::TypeParameterId> = declared_param_ids
        .iter()
        .copied()
        .filter(|id| !ambient_ids.contains(id))
        .collect();

    let mut bindings: Vec<(crate::types::TypeParameterId, TypeId)> = own_ids
        .iter()
        .zip(explicit_type_args.iter())
        .map(|(&id, &explicit)| (id, explicit))
        .collect();
    let mut locked: Vec<crate::types::TypeParameterId> =
        bindings.iter().map(|(id, _)| *id).collect();
    for &id in &ambient_ids {
        if let Some(node) = ctx.namespace.type_param_node(id) {
            bindings.push((id, node));
            locked.push(id);
        }
    }

    let mut inference = GenericInferenceSession::new(bindings, locked);
    for (index, arg_type) in arg_types.iter().enumerate() {
        let Some(arg_type) = arg_type else { continue };
        if let Some(param_type) =
            GenericSolver::expected_param_type(&ctx.arena, &function_type.params, index)
        {
            inference.infer_argument(
                &mut ctx.arena,
                param_type,
                *arg_type,
                &mut ctx.relation_cache,
            );
        }
    }
    let mut bindings = inference.finish(&mut ctx.arena, &mut ctx.relation_cache);

    // Defaults are consulted only after argument inference: an inferred candidate wins,
    // an explicit argument is already locked, and a parameter with no candidate receives
    // its default. Processing in declaration order lets `<T, U = T>` substitute T before
    // U is added. Unspecified parameters without defaults retain the existing unknown
    // recovery behavior at final substitution.
    for parameter in &function_type.type_params {
        if ambient_ids.contains(&parameter.id) || bindings.iter().any(|(id, _)| *id == parameter.id)
        {
            continue;
        }
        if let Some(default) = parameter.default {
            let default =
                GenericSolver::substitute_bound_type_params(&mut ctx.arena, default, &bindings);
            bindings.push((parameter.id, default));
        }
    }

    // Checked once per call, after every argument candidate has been resolved and
    // defaults applied, before the per-argument assignability loop: every declared
    // generic parameter's constraint is checked, including an unused `<T extends X>`.
    let mut constraints = Vec::new();
    for parameter in &function_type.type_params {
        if let Some(constraint) = parameter.constraint
            && !constraints.iter().any(|(id, _, _)| *id == parameter.id)
        {
            constraints.push((parameter.id, parameter.name.clone(), constraint));
        }
    }
    for param in &function_type.params {
        GenericSolver::collect_generic_param_constraints(
            &ctx.arena,
            param.type_id,
            &mut constraints,
        );
    }
    GenericSolver::collect_generic_param_constraints(
        &ctx.arena,
        function_type.return_type,
        &mut constraints,
    );

    for (id, _, constraint) in &constraints {
        if ambient_ids.contains(id) {
            continue;
        }
        let Some(&(_, bound)) = bindings.iter().find(|(bound_id, _)| bound_id == id) else {
            continue;
        };

        // A bound can be written in terms of the other parameters
        // (`U extends T[]`, `T extends Comparable<T>`), so it has to see their
        // inferred types before the comparison; checked raw, `number[]` would be
        // compared against the generic `T[]` and rejected. Parameters with no
        // binding are kept as they are, and a bound that still mentions one has
        // nothing concrete to compare against, so it is left alone (the same
        // "uninferred is not an error" stance as the skip above).
        let constraint =
            GenericSolver::substitute_bound_type_params(&mut ctx.arena, *constraint, &bindings);
        if GenericSolver::contains_type_param(&ctx.arena, constraint) {
            continue;
        }
        if !ctx.semantic().is_assignable(bound, constraint) {
            // A type argument the caller wrote out is TS2344. One that was inferred is
            // reported by tsc as the argument that does not fit the parameter's
            // constraint (TS2345).
            let written = own_ids
                .iter()
                .position(|own| own == id)
                .is_some_and(|position| position < explicit_type_args.len());
            let message = if written {
                crate::diagnostic_messages::messages::type_argument_constraint_violation(
                    &ctx.arena, bound, constraint,
                )
            } else {
                crate::diagnostic_messages::messages::argument_not_assignable(
                    &ctx.arena, bound, constraint,
                )
            };
            ctx.error(message, span);
        }
    }

    for (index, arg) in arguments.iter().enumerate() {
        let Some(arg_expr) = arg.as_expression() else {
            continue;
        };
        let Some(arg_type) = arg_types[index] else {
            continue;
        };
        let Some(param_type) =
            GenericSolver::expected_param_type(&ctx.arena, &function_type.params, index)
        else {
            continue;
        };
        let expected = GenericSolver::substitute_type_params(&mut ctx.arena, param_type, &bindings);
        if !ctx.is_assignable_instantiating(arg_type, expected) {
            // tsc reports a bad property of an object literal argument on the property
            // itself, as a plain assignability error; only an argument it cannot take
            // apart gets the `Argument of type ...` message.
            if !super::elaborate::elaborate_mismatch(arg_expr, arg_type, expected, ctx) {
                ctx.error(
                    crate::diagnostic_messages::messages::argument_not_assignable(
                        &ctx.arena, arg_type, expected,
                    ),
                    arg_expr.span(),
                );
            }
        } else {
            check_excess_properties(arg_expr, expected, ctx);
        }
    }

    GenericSolver::substitute_type_params(&mut ctx.arena, function_type.return_type, &bindings)
}

fn is_array_literal(expr: &Expression) -> bool {
    match expr {
        Expression::ArrayExpression(_) => true,
        Expression::ParenthesizedExpression(inner) => is_array_literal(&inner.expression),
        _ => false,
    }
}

fn arity_message(
    required: usize,
    max: Option<usize>,
    got: usize,
) -> crate::diagnostic_messages::DiagnosticMessage {
    use crate::diagnostic_messages::messages;
    match max {
        Some(max) if max == required => messages::argument_arity_exact(required, got),
        Some(max) => messages::argument_arity_range(required, max, got),
        None => messages::argument_arity_at_least(required, got),
    }
}
