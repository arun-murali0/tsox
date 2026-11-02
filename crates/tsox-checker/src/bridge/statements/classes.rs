use std::collections::HashSet;

use oxc_ast::ast::{
    AssignmentOperator, AssignmentTarget, ClassElement, Expression, MethodDefinitionKind,
    PropertyDefinitionType, PropertyKey, Statement,
};
use oxc_semantic::Scoping;
use oxc_span::GetSpan;

use crate::namespace::Resolution;
use crate::type_annotation::{resolve_function_params, resolve_type_annotation};
use crate::types::Type;

use super::super::context::CheckContext;
use super::super::expressions::infer_expression_type;
use super::support::report_implicit_any_params;
use super::{bind_params, check_statement};

fn has_getter_named(class: &oxc_ast::ast::Class<'_>, key: &oxc_ast::ast::PropertyKey<'_>) -> bool {
    let Some(name) = key.static_name() else {
        return false;
    };
    class.body.body.iter().any(|element| {
        matches!(
            element,
            ClassElement::MethodDefinition(m)
                if matches!(m.kind, oxc_ast::ast::MethodDefinitionKind::Get)
                    && m.key.static_name().is_some_and(|other| other == name)
        )
    })
}

// `private m()` and `#m()` both: the two ways a class member is private to its class.
fn is_private_member(method: &oxc_ast::ast::MethodDefinition<'_>) -> bool {
    matches!(
        method.accessibility,
        Some(oxc_ast::ast::TSAccessibility::Private)
    ) || matches!(method.key, PropertyKey::PrivateIdentifier(_))
}

pub(super) fn check_class_declaration(
    class: &oxc_ast::ast::Class<'_>,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) {
    // Reported up front, before anything below can bail out, so a class this
    // checker treats as wholly unsupported still gets its untyped parameters
    // flagged the way tsc would.
    for element in &class.body.body {
        if let ClassElement::MethodDefinition(method) = element {
            // Problem: `set p(v) {}` next to `get p()` read as an implicit any, but tsc types
            // the setter's parameter from the getter. Picked: skip a setter that has a
            // getter of the same name in the class. A setter alone is still reported.
            if matches!(method.kind, oxc_ast::ast::MethodDefinitionKind::Set)
                && has_getter_named(class, &method.key)
            {
                continue;
            }
            // Problem: `declare class C { private m(a); }` read as an implicit any, but tsc
            // never reports one for a parameter of a private member in an ambient context
            // (declarationBelongsToPrivateAmbientMember in its checker): a private member
            // is not part of the shape callers see, so its types are not asked for.
            // Picked: skip a private method or constructor, `private` or `#name`, of a class
            // that is ambient by `declare` or by sitting in a `declare namespace`/`module`.
            if is_private_member(method) && (class.declare || ctx.ambient_depth > 0) {
                continue;
            }
            // A setter alone gets TS7032 on its name as well as TS7006 on the parameter,
            // in that order, the way tsc reports it.
            if matches!(method.kind, oxc_ast::ast::MethodDefinitionKind::Set)
                && let Some(name) = method.key.static_name()
                && let Some(first) = method.value.params.items.first()
                && first.type_annotation.is_none()
                && first.initializer.is_none()
            {
                ctx.error(
                    crate::diagnostic_messages::messages::setter_property_implicitly_any(&name),
                    method.key.span(),
                );
            }
            report_implicit_any_params(&method.value.params, ctx);
        }
    }

    // Same reasoning: a field with no initializer is a problem whether or not the
    // rest of the class could be resolved.
    report_uninitialized_properties(class, ctx);

    let Some(name) = class.id.as_ref() else {
        return;
    };
    // The class's already-resolved instance shape, built once during the declare
    // pass (see resolve_class in namespace.rs). Instance methods below reuse the
    // signatures baked into this shape rather than re-resolving them, the same
    // pattern check_function_declaration uses for plain functions. A generic
    // class's own type-parameter scope (`class Box<T>`) is pushed below, right
    // after instance_type is resolved, so both the constructor and instance
    // method bodies checked in this function can also refer to T directly.
    let instance_type = match ctx.namespace.resolve(&name.name, &mut ctx.arena) {
        Resolution::Resolved(type_id) => type_id,
        Resolution::Circular | Resolution::NotFound => {
            ctx.warning(
                crate::diagnostic_messages::messages::unimplemented_class_shape(&name.name),
                class.span(),
            );
            return;
        }
    };
    let Type::Object(instance_object) = ctx.arena.get(instance_type).clone() else {
        return;
    };

    let outer_class_instance = ctx.current_class_instance.replace(instance_type);

    // Pushes the class's own `<T, ...>` scope for the check-time pass over the body.
    // resolve_class resolved the class at declare time, but that scope is gone by
    // the time bodies are checked, so T has to be pushed again here.
    // Without it the constructor's params, re-resolved below against ctx.namespace,
    // fail to find T and the constructor body is silently skipped. Instance method
    // params come pre-resolved from instance_object, so those bodies are still
    // checked, but any expression in them that resolves T standalone falls back to
    // the unresolved path.
    // push_decl_type_params, not push_type_params: a class's `<T, ...>` list is a
    // TSTypeParameterDeclaration (the same shape as an interface's or alias's),
    // while push_type_params takes an oxc Function node.
    let type_param_scope = ctx
        .namespace
        .push_decl_type_params(&mut ctx.arena, class.type_parameters.as_deref());

    // The constructor is not part of the instance's own structural type (nothing
    // outside the class calls it as `instance.constructor(...)`), so unlike an
    // instance method below, its parameter types are resolved fresh here rather
    // than reused from instance_object.
    if let Some(ctor) = super::super::declare::find_constructor(class) {
        if let Some(ctor_body) = &ctor.body {
            if let Some(params) =
                resolve_function_params(&ctor.params, &mut ctx.namespace, &mut ctx.arena)
            {
                bind_params(&ctor.params, &params, scoping, ctx);
                // Like a function declaration, a constructor body starts with no
                // narrowing and leaves none behind.
                let outer_narrow = std::mem::take(&mut ctx.narrow);
                for body_stmt in &ctor_body.statements {
                    check_statement(body_stmt, scoping, ctx);
                }
                ctx.narrow = outer_narrow;
            }
        }
    }

    for element in &class.body.body {
        match element {
            ClassElement::MethodDefinition(method)
                if !method.r#static && method.kind == MethodDefinitionKind::Method =>
            {
                let PropertyKey::StaticIdentifier(key) = &method.key else {
                    continue;
                };
                let Some(method_body) = &method.value.body else {
                    continue;
                };

                // Reuses the method's signature from instance_object, computed
                // once during resolve_class, rather than re-resolving it here.
                let Some(method_property) = instance_object
                    .properties
                    .iter()
                    .find(|p| p.name.as_ref() == key.name.as_str())
                else {
                    continue;
                };
                let Type::Function(method_type) = ctx.arena.get(method_property.type_id).clone()
                else {
                    continue;
                };

                bind_params(&method.value.params, &method_type.params, scoping, ctx);
                let return_scope = ctx.enter_function_return_scope(
                    Some(method_type.return_type),
                    false,
                    method.value.r#async,
                );
                let outer_narrow = std::mem::take(&mut ctx.narrow);
                for body_stmt in &method_body.statements {
                    check_statement(body_stmt, scoping, ctx);
                }
                ctx.narrow = outer_narrow;
                ctx.leave_return_scope(return_scope);
            }

            // A static method lives on the class itself, not on instances, so it
            // has no entry in instance_object the way an instance method does,
            // and its signature is resolved fresh here instead.
            ClassElement::MethodDefinition(method)
                if method.r#static && method.kind == MethodDefinitionKind::Method =>
            {
                let Some(method_body) = &method.value.body else {
                    continue;
                };

                let (params, _is_untyped) = match resolve_function_params(
                    &method.value.params,
                    &mut ctx.namespace,
                    &mut ctx.arena,
                ) {
                    Some(params) => (params, false),
                    None => (Vec::new(), true),
                };
                bind_params(&method.value.params, &params, scoping, ctx);

                let return_type =
                    method.value.return_type.as_ref().and_then(|rt| {
                        resolve_type_annotation(rt, &mut ctx.namespace, &mut ctx.arena)
                    });
                let return_scope =
                    ctx.enter_function_return_scope(return_type, false, method.value.r#async);
                // `this` inside a static method is not the instance; tsc types
                // it as the class's constructor type, which this checker does
                // not model. Clearing current_class_instance to None for just
                // this body (rather than leaving the outer_class_instance value
                // set above in place) stops `this` from wrongly resolving to the
                // instance type here; it falls into the same Error-sentinel,
                // no-diagnostic case a `this` of legitimately unknown origin
                // already gets (see the doc comment on Expression::ThisExpression
                // in expressions/mod.rs), rather than being silently wrong.
                let outer_class_instance_for_this = ctx.current_class_instance.take();
                let outer_narrow = std::mem::take(&mut ctx.narrow);
                for body_stmt in &method_body.statements {
                    check_statement(body_stmt, scoping, ctx);
                }
                ctx.narrow = outer_narrow;
                ctx.current_class_instance = outer_class_instance_for_this;
                ctx.leave_return_scope(return_scope);
            }

            ClassElement::MethodDefinition(method) if method.r#static => {
                ctx.warning(
                    crate::diagnostic_messages::messages::unimplemented_static_accessor(),
                    method.span(),
                );
            }

            ClassElement::PropertyDefinition(prop) if prop.r#static => {
                let Some(initializer) = &prop.value else {
                    continue;
                };
                let actual = infer_expression_type(initializer, scoping, ctx);
                if let Some(annotation) = &prop.type_annotation {
                    if let Some(declared) =
                        resolve_type_annotation(annotation, &mut ctx.namespace, &mut ctx.arena)
                    {
                        if !ctx.is_assignable_instantiating(actual, declared) {
                            ctx.error(
                                crate::diagnostic_messages::messages::static_field_initializer_mismatch(
                                    &ctx.arena, actual, declared,
                                ),
                                initializer.span(),
                            );
                        }
                    }
                }
            }

            _ => {}
        }
    }

    ctx.namespace.pop_type_params(type_param_scope);
    ctx.current_class_instance = outer_class_instance;
}

// TS2564, under strictPropertyInitialization: an instance property declared with
// a type but given no initializer must be assigned in the constructor, unless
// its type already allows `undefined`.
//
// The assignment check is a real, if narrow, definite-assignment analysis (see
// definitely_assigned_names below): sequential statements and if/else are
// understood, so a guard-clause pattern (`if (!x) throw ...; this.value = 1;`)
// correctly counts, and an assignment inside only one arm of an if with no
// else, or an if/else where only one branch assigns, correctly does not. A
// loop, switch, or try is not analyzed per-branch and never contributes an
// assignment, narrower than tsc, but only in the direction of reporting a
// real gap rather than a false positive on correct code. Assignments inside a
// nested function or arrow also never count, matching tsc, since they may run
// after the constructor has finished.
//
// Nothing is reported for a class that is ambient, whether by its own `declare` or by sitting
// in a `declare namespace` / `declare module`.
//
// A property is skipped, without an error, when any of these hold: it is static
// or `declare`d or abstract, it has an initializer, it is optional (`x?: T`), it
// carries the definite assignment assertion (`x!: T`), it has no type annotation
// (that is a different error, TS7008), or its annotation cannot be resolved (so
// whether `undefined` is allowed is unknown).
fn report_uninitialized_properties(
    class: &oxc_ast::ast::Class<'_>,
    ctx: &mut CheckContext<'_, '_>,
) {
    // A class in a `declare namespace` or `declare module "x"` is ambient without a
    // `declare` of its own, and an ambient class has no constructor body to assign in.
    if class.declare || ctx.ambient_depth > 0 {
        return;
    }

    let assigned_names = super::super::declare::find_constructor(class)
        .and_then(|ctor| ctor.body.as_ref())
        .map(|body| definitely_assigned_names(&body.statements))
        .unwrap_or_default();

    for element in &class.body.body {
        let ClassElement::PropertyDefinition(prop) = element else {
            continue;
        };
        if prop.r#static
            || prop.declare
            || prop.optional
            || prop.definite
            || prop.value.is_some()
            || matches!(
                prop.r#type,
                PropertyDefinitionType::TSAbstractPropertyDefinition
            )
        {
            continue;
        }
        let PropertyKey::StaticIdentifier(key) = &prop.key else {
            continue;
        };
        let Some(annotation) = &prop.type_annotation else {
            continue;
        };
        if assigned_names.contains(key.name.as_str()) {
            continue;
        }
        let Some(declared) =
            resolve_type_annotation(annotation, &mut ctx.namespace, &mut ctx.arena)
        else {
            continue;
        };

        let undefined = ctx.arena.undefined();
        if ctx.semantic().is_assignable(undefined, declared) {
            continue;
        }
        ctx.error(
            crate::diagnostic_messages::messages::property_not_initialized(&key.name),
            key.span,
        );
    }
}

// Property names definitely assigned (`this.name = ...`) by every path through
// a sequence of statements. Union across the sequence; each statement here
// runs unconditionally after the ones before it, so whatever any one of them
// definitely assigns is definitely assigned by the end of the sequence too.
// Does not descend into a nested function or arrow (see names_assigned_by).
fn definitely_assigned_names(stmts: &[Statement]) -> HashSet<String> {
    let mut assigned = HashSet::new();
    for stmt in stmts {
        assigned.extend(names_assigned_by(stmt));
    }
    assigned
}

// What one statement, on its own, definitely assigns. A loop, switch, try, or
// anything else not matched here contributes nothing, not because it never
// assigns anything, but because this analysis does not attempt to reason about
// it, the same conservative "leave it a gap rather than risk a false positive"
// stance the whole function takes.
fn names_assigned_by(stmt: &Statement) -> HashSet<String> {
    match stmt {
        Statement::ExpressionStatement(expr_stmt) => {
            this_property_assigned_by(&expr_stmt.expression)
                .into_iter()
                .collect()
        }
        Statement::BlockStatement(block) => definitely_assigned_names(&block.body),
        Statement::IfStatement(if_stmt) => {
            let Some(alternate) = &if_stmt.alternate else {
                // No else: the implicit empty else assigns nothing, so nothing
                // is definite after the `if` regardless of what the consequent
                // assigns internally.
                return HashSet::new();
            };
            let consequent_exits = super::support::statement_always_exits(&if_stmt.consequent);
            let alternate_exits = super::support::statement_always_exits(alternate);
            match (consequent_exits, alternate_exits) {
                // Both branches always exit: there is no "falling out" of this
                // `if` for anything after it to run through.
                (true, true) => HashSet::new(),
                // Only the branch that does not always exit can reach code
                // after the `if`, so only its assignments are definite.
                (true, false) => names_assigned_by(alternate),
                (false, true) => names_assigned_by(&if_stmt.consequent),
                // Neither exits: a name is definite only if both branches
                // assign it.
                (false, false) => names_assigned_by(&if_stmt.consequent)
                    .intersection(&names_assigned_by(alternate))
                    .cloned()
                    .collect(),
            }
        }
        _ => HashSet::new(),
    }
}

// `this.name = value` (plain assignment only, not `+=` and friends) at the top
// level of one expression. Never looks inside a nested function or arrow
// expression, since an assignment there may run after the constructor has
// already finished, matching tsc.
fn this_property_assigned_by(expr: &Expression) -> Option<String> {
    let Expression::AssignmentExpression(assign) = expr else {
        return None;
    };
    if assign.operator != AssignmentOperator::Assign {
        return None;
    }
    let AssignmentTarget::StaticMemberExpression(member) = &assign.left else {
        return None;
    };
    if !matches!(member.object, Expression::ThisExpression(_)) {
        return None;
    }
    Some(member.property.name.to_string())
}
