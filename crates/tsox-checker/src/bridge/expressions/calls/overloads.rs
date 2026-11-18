//! Overload selection: candidate fitting, arity and no-match reporting.
use super::*;

pub(in crate::bridge) enum PickedCallable {
    // The member to check the call against in the ordinary way.
    Member(TypeId),
    // The call has been reported already; this is the type it is left with.
    Reported(TypeId),
}

// How one function signature meets a call's arguments.
pub(in crate::bridge) enum Fit {
    Fits,
    WrongArity,
    // The first argument that is not assignable to its inferred parameter.
    WrongArgument {
        index: usize,
        parameter: TypeId,
        argument: TypeId,
    },
    // The explicit type argument list cannot instantiate this signature. The ordinary
    // call checker owns the TS2558 diagnostic if no other overload accepts the list.
    WrongTypeArguments,
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
// Candidate probing infers ordinary argument types once and evaluates callbacks against
// each candidate's contextual signature. Provisional diagnostics are discarded; the final
// selected-signature check re-infers the arguments and owns all visible diagnostics. A
// spread argument is not probed: its length is not known, so the first function member is
// used.
pub(in crate::bridge) fn pick_callable_member(
    callee_type: TypeId,
    arguments: &[oxc_ast::ast::Argument<'_>],
    call_span: Span,
    explicit_type_args: &[TypeId],
    callee_is_declaration: bool,
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
            // Ordinary arguments have candidate-independent types and are inferred once.
            // Callbacks, and object and array literals, are left for each candidate's
            // contextual probe below: a callback's parameter and return types can decide
            // whether a generic overload applies, and a literal reads differently against
            // each candidate (`{ kind: "numeric" }` keeps its literal where the expected
            // type holds one, and widens to `string` where it does not).
            let mut argument_types = Vec::with_capacity(arguments.len());
            for argument in arguments {
                let Some(expression) = argument.as_expression() else {
                    argument_types.push(None);
                    continue;
                };
                argument_types.push(if is_candidate_typed(expression) {
                    None
                } else {
                    Some(infer_expression_type(expression, scoping, ctx))
                });
            }
            ctx.diagnostics.truncate(reported_before);

            let fits: Vec<(TypeId, Fit)> = callable
                .iter()
                .map(|&member| {
                    let fit = match ctx.arena.get(member).clone() {
                        Type::Function(function) => fit_of_candidate(
                            ctx,
                            &function,
                            &argument_types,
                            arguments,
                            scoping,
                            explicit_type_args,
                            callee_is_declaration,
                        ),
                        _ => Fit::WrongArity,
                    };
                    (member, fit)
                })
                .collect();
            ctx.diagnostics.truncate(reported_before);
            if let Some((member, _)) = fits.iter().find(|(_, fit)| matches!(fit, Fit::Fits)) {
                return PickedCallable::Member(*member);
            }

            let rejected: Vec<(TypeId, usize, TypeId, TypeId)> = fits
                .iter()
                .filter_map(|(member, fit)| match fit {
                    Fit::WrongArgument {
                        index,
                        parameter,
                        argument,
                    } => Some((*member, *index, *parameter, *argument)),
                    _ => None,
                })
                .collect();
            match rejected.as_slice() {
                [] if fits
                    .iter()
                    .any(|(_, fit)| matches!(fit, Fit::WrongTypeArguments)) =>
                {
                    // Keep one candidate so check_callable can issue the established
                    // TS2558 type-argument-count diagnostic. If another overload accepts
                    // the written list, it would already have been selected above.
                    let candidate = fits
                        .iter()
                        .find(|(_, fit)| matches!(fit, Fit::WrongTypeArguments))
                        .map(|(member, _)| *member)
                        .unwrap_or(callable[0]);
                    PickedCallable::Member(candidate)
                }
                [] => report_arity_over_all(&callable, arguments, call_span, ctx),
                [(only, ..)] => PickedCallable::Member(*only),
                _ => report_no_overload_matches(&rejected, arguments, call_span, ctx),
            }
        }
    }
}

// Every member fails on the number of arguments: one TS2554 over the range the members allow
// together, from the fewest any of them needs to the most any of them takes (open-ended when
// one has a rest parameter).
pub(in crate::bridge) fn report_arity_over_all(
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
pub(in crate::bridge) fn report_no_overload_matches(
    rejected: &[(TypeId, usize, TypeId, TypeId)],
    arguments: &[oxc_ast::ast::Argument<'_>],
    call_span: Span,
    ctx: &mut CheckContext<'_, '_>,
) -> PickedCallable {
    let mut text = String::from("No overload matches this call.");
    for (number, (member, _index, parameter, argument)) in rejected.iter().enumerate() {
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
            &ctx.arena, *argument, *parameter,
        );
        for line in reason.text.lines() {
            text.push_str("\n    ");
            text.push_str(line);
        }
    }

    // Overloads that all failed on the same argument blame that argument, so the error
    // sits on it. Failing on different ones, no single argument is to blame, so it sits on
    // the call.
    let same_argument = rejected
        .iter()
        .all(|(_, index, _, _)| *index == rejected[0].1);
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
pub(in crate::bridge) fn expand_tuple_rest(
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

// Evaluate one overload in an independent inference session. A generic parameter is
// never treated as an automatic match: candidates are inferred from this call's arguments,
// defaults and constraints are applied, and the resulting parameter types are checked just
// like a non-generic signature. The session is local to this candidate, so a failed
// overload cannot leave bindings behind for the next one.
pub(in crate::bridge) fn fit_of_candidate(
    ctx: &mut CheckContext<'_, '_>,
    function: &FunctionType,
    initial_argument_types: &[Option<TypeId>],
    arguments: &[oxc_ast::ast::Argument<'_>],
    scoping: &Scoping,
    explicit_type_args: &[TypeId],
    callee_is_declaration: bool,
) -> Fit {
    // A rest parameter typed as a tuple stands for its elements, as a fixed run of parameters.
    let expanded = expand_tuple_rest(&ctx.arena, function);
    let function = expanded.as_ref().unwrap_or(function);
    let diagnostics_before = ctx.diagnostics.len();
    let result = (|| {
        if function.is_untyped {
            return Fit::Fits;
        }

        let required = function
            .params
            .iter()
            .filter(|param| !param.optional && !param.rest)
            .count();
        let rest = function.params.last().filter(|param| param.rest);
        if arguments.len() < required {
            return Fit::WrongArity;
        }
        if rest.is_none() && arguments.len() > function.params.len() {
            return Fit::WrongArity;
        }

        // Keep declaration order and include parameters that occur only in constraints,
        // defaults, or the return type. This mirrors the final call checker, rather than
        // discovering only parameters that happen to occur in an input position.
        let mut declared_param_ids = Vec::new();
        for parameter in &function.type_params {
            if !declared_param_ids.contains(&parameter.id) {
                declared_param_ids.push(parameter.id);
            }
        }
        for parameter in &function.params {
            GenericSolver::ordered_generic_param_ids(
                &ctx.arena,
                parameter.type_id,
                &mut declared_param_ids,
            );
        }
        GenericSolver::ordered_generic_param_ids(
            &ctx.arena,
            function.return_type,
            &mut declared_param_ids,
        );

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

        if !explicit_type_args.is_empty() {
            let required_type_args = function
                .type_params
                .iter()
                .rposition(|parameter| parameter.default.is_none())
                .map_or(0, |index| index + 1);
            if explicit_type_args.len() < required_type_args
                || explicit_type_args.len() > function.type_params.len()
                || explicit_type_args.len() > own_ids.len()
            {
                return Fit::WrongTypeArguments;
            }
        }

        let bindings: Vec<(crate::types::TypeParameterId, TypeId)> = own_ids
            .iter()
            .zip(explicit_type_args.iter())
            .map(|(&id, &explicit)| (id, explicit))
            .collect();
        let mut locked: Vec<crate::types::TypeParameterId> =
            bindings.iter().map(|(id, _)| *id).collect();
        let mut bindings = bindings;
        for &id in &ambient_ids {
            if let Some(node) = ctx.namespace.type_param_node(id) {
                bindings.push((id, node));
                locked.push(id);
            }
        }

        let mut inference = GenericInferenceSession::new(bindings, locked);
        let mut argument_types = initial_argument_types.to_vec();
        for (index, argument_type) in argument_types.iter().enumerate() {
            let Some(argument_type) = argument_type else {
                continue;
            };
            if let Some(parameter_type) =
                GenericSolver::expected_param_type(&ctx.arena, &function.params, index)
            {
                inference.infer_argument(
                    &mut ctx.arena,
                    parameter_type,
                    *argument_type,
                    &mut ctx.relation_cache,
                );
            }
        }

        // Contextually type each callback against this candidate's preliminary bindings.
        // This both avoids treating an untyped callback as a wildcard and lets callback
        // return values contribute candidates to generic return parameters. The final
        // selected-signature pass checks the callback again and owns visible diagnostics.
        let mut contextual_bindings = inference
            .clone()
            .finish(&mut ctx.arena, &mut ctx.relation_cache);
        for parameter in &function.type_params {
            if ambient_ids.contains(&parameter.id)
                || contextual_bindings
                    .iter()
                    .any(|(id, _)| *id == parameter.id)
            {
                continue;
            }
            let fallback = parameter
                .default
                .or(parameter.constraint)
                .unwrap_or_else(|| ctx.arena.unknown());
            let fallback = GenericSolver::substitute_bound_type_params(
                &mut ctx.arena,
                fallback,
                &contextual_bindings,
            );
            if !GenericSolver::contains_type_param(&ctx.arena, fallback) {
                contextual_bindings.push((parameter.id, fallback));
            }
        }

        for (index, argument) in arguments.iter().enumerate() {
            let Some(expression) = argument.as_expression() else {
                continue;
            };
            if !is_candidate_typed(expression) {
                continue;
            }
            let is_callback = matches!(
                expression,
                Expression::ArrowFunctionExpression(_) | Expression::FunctionExpression(_)
            );
            let Some(parameter_type) =
                GenericSolver::expected_param_type(&ctx.arena, &function.params, index)
            else {
                continue;
            };
            let contextual_type = GenericSolver::substitute_bound_type_params(
                &mut ctx.arena,
                parameter_type,
                &contextual_bindings,
            );
            let prior_context = ctx.contextual.replace(contextual_type);
            let inferred = infer_expression_type(expression, scoping, ctx);
            ctx.contextual = prior_context;
            argument_types[index] = Some(inferred);
            // A callback is evidence of the strength the main call path gives it; a literal
            // is an ordinary argument that was only typed late, so it counts as one.
            let priority = if is_callback {
                InferencePriority::Contextual
            } else {
                InferencePriority::Normal
            };
            inference.infer_argument_with_priority(
                &mut ctx.arena,
                parameter_type,
                inferred,
                priority,
                &mut ctx.relation_cache,
            );
        }

        let mut bindings = inference.finish(&mut ctx.arena, &mut ctx.relation_cache);

        // Apply defaults in declaration order, so a later default can refer to an earlier
        // inferred or defaulted parameter (`<T, U = T>`).
        for parameter in &function.type_params {
            if ambient_ids.contains(&parameter.id)
                || bindings.iter().any(|(id, _)| *id == parameter.id)
            {
                continue;
            }
            if let Some(default) = parameter.default {
                let default =
                    GenericSolver::substitute_bound_type_params(&mut ctx.arena, default, &bindings);
                bindings.push((parameter.id, default));
            }
        }

        // Constraints are part of applicability. A generic overload whose inferred T
        // violates `T extends C` must not mask a later overload that accepts the input.
        let mut constraints = Vec::new();
        for parameter in &function.type_params {
            if let Some(constraint) = parameter.constraint
                && !constraints.iter().any(|(id, _, _)| *id == parameter.id)
            {
                constraints.push((parameter.id, parameter.name.clone(), constraint));
            }
        }
        for parameter in &function.params {
            GenericSolver::collect_generic_param_constraints(
                &ctx.arena,
                parameter.type_id,
                &mut constraints,
            );
        }
        GenericSolver::collect_generic_param_constraints(
            &ctx.arena,
            function.return_type,
            &mut constraints,
        );

        for (id, _, raw_constraint) in &constraints {
            if ambient_ids.contains(id) {
                continue;
            }
            let Some(&(_, inferred)) = bindings.iter().find(|(bound_id, _)| bound_id == id) else {
                continue;
            };
            let inferred =
                GenericSolver::substitute_bound_type_params(&mut ctx.arena, inferred, &bindings);
            let constraint = GenericSolver::substitute_bound_type_params(
                &mut ctx.arena,
                *raw_constraint,
                &bindings,
            );
            if GenericSolver::contains_type_param(&ctx.arena, inferred)
                || GenericSolver::contains_type_param(&ctx.arena, constraint)
                || crate::subtyping::is_subtype(&ctx.arena, inferred, constraint)
            {
                continue;
            }

            // Associate a failed inferred constraint with the argument that supplied the
            // parameter, giving TS2769 a useful branch rather than a generic wildcard fit.
            for index in 0..argument_types.len() {
                let Some(argument_type) = argument_types[index] else {
                    continue;
                };
                if let Some(parameter_type) =
                    GenericSolver::expected_param_type(&ctx.arena, &function.params, index)
                {
                    let mut parameter_ids = Vec::new();
                    GenericSolver::ordered_generic_param_ids(
                        &ctx.arena,
                        parameter_type,
                        &mut parameter_ids,
                    );
                    if parameter_ids.contains(id) {
                        return Fit::WrongArgument {
                            index,
                            parameter: constraint,
                            argument: argument_type,
                        };
                    }
                }
            }
            return Fit::WrongTypeArguments;
        }

        for (index, argument_type) in argument_types.iter().enumerate() {
            let Some(argument_type) = argument_type else {
                continue;
            };
            let Some(parameter_type) =
                GenericSolver::expected_param_type(&ctx.arena, &function.params, index)
            else {
                continue;
            };
            let expected =
                GenericSolver::substitute_type_params(&mut ctx.arena, parameter_type, &bindings);
            if !crate::subtyping::is_subtype(&ctx.arena, *argument_type, expected) {
                return Fit::WrongArgument {
                    index,
                    parameter: expected,
                    argument: *argument_type,
                };
            }
        }

        Fit::Fits
    })();
    ctx.diagnostics.truncate(diagnostics_before);
    result
}

/// Whether an overload candidate types this argument for itself: a function, or an object or
/// array literal, whose type depends on what the candidate expects there.
pub(in crate::bridge) fn is_candidate_typed(expr: &Expression) -> bool {
    matches!(
        expr,
        Expression::ArrowFunctionExpression(_)
            | Expression::FunctionExpression(_)
            | Expression::ObjectExpression(_)
            | Expression::ArrayExpression(_)
    )
}

/// Whether an expression holds a function whose parameters need a contextual type, the way
/// tsc defines a context-sensitive argument: a function or arrow with a parameter that has
/// no annotation, found directly or inside an object or array literal. Only such arguments
/// are put off until the other arguments have bound the type parameters; an object whose
/// callbacks are all annotated is inferred in the first pass like any ordinary argument.
pub(in crate::bridge) fn is_context_sensitive(expr: &Expression) -> bool {
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
