use oxc_ast::ast::{BindingPattern, Expression, VariableDeclarationKind, VariableDeclarator};
use oxc_semantic::Scoping;
use oxc_span::GetSpan;

use crate::arena::TypeId;
use crate::type_annotation::resolve_type_annotation;
use crate::types::Type;

use super::super::context::CheckContext;
use super::super::expressions::{
    check_excess_properties, infer_expression_type, narrow_on_assignment, report_mismatch,
};
use super::bind_pattern;
use super::support::{find_unresolved_type_name, report_implicit_any_params};

/// What we know about a declarator's declared type after attempting to
/// resolve its type annotation (if any). Kept separate from the inferred
/// type of its initializer so the two can be compared and merged explicitly
/// per declarator kind, rather than collapsing "no annotation" and
/// "annotation present but unresolvable" into the same case.
enum AnnotationOutcome {
    /// No type annotation was written on this declarator.
    Absent,
    /// A type annotation was written and resolved successfully.
    Resolved(TypeId),
    /// A type annotation was written but could not be resolved (unknown
    /// name, or a definition tsox doesn't fully understand yet).
    Unresolvable,
}

pub(super) fn check_variable_declaration(
    decl: &oxc_ast::ast::VariableDeclaration,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) {
    for declarator in &decl.declarations {
        match &declarator.id {
            BindingPattern::BindingIdentifier(id) => {
                check_identifier_declarator(decl, declarator, id, scoping, ctx);
            }
            BindingPattern::ObjectPattern(_) | BindingPattern::ArrayPattern(_) => {
                check_destructured_declarator(decl.kind, declarator, scoping, ctx);
            }

            BindingPattern::AssignmentPattern(_) => {}
        }
    }
}

fn check_identifier_declarator(
    decl: &oxc_ast::ast::VariableDeclaration,
    declarator: &VariableDeclarator,
    id: &oxc_ast::ast::BindingIdentifier,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) {
    let decl_kind = decl.kind;
    let annotation_outcome = match &declarator.type_annotation {
        None => AnnotationOutcome::Absent,
        Some(annotation) => {
            match resolve_type_annotation(annotation, &mut ctx.namespace, &mut ctx.arena) {
                Some(type_id) => AnnotationOutcome::Resolved(type_id),
                None => AnnotationOutcome::Unresolvable,
            }
        }
    };

    // With no annotation on the variable there is no contextual type for the
    // function on the right, so any untyped parameter of it is a genuine
    // implicit any. Reported before the body is inferred so the diagnostics come
    // out in source order.
    if declarator.type_annotation.is_none() {
        match &declarator.init {
            Some(Expression::ArrowFunctionExpression(arrow)) => {
                report_implicit_any_params(&arrow.params, ctx);
            }
            Some(Expression::FunctionExpression(func)) => {
                report_implicit_any_params(&func.params, ctx);
                // Nothing supplies a contextual `this` here either, so `this`
                // inside this function is an implicit any.
                ctx.next_function_has_no_this = true;
            }
            _ => {}
        }
    }

    let inferred_type = declarator.init.as_ref().map(|init| {
        // The annotation is the contextual type of the initializer.
        if let AnnotationOutcome::Resolved(declared) = &annotation_outcome {
            ctx.contextual = Some(*declared);
        }
        infer_expression_type(init, scoping, ctx)
    });

    match (annotation_outcome, inferred_type) {
        (AnnotationOutcome::Unresolvable, _) => {
            let unknown_name = declarator.type_annotation.as_ref().and_then(|annotation| {
                find_unresolved_type_name(
                    &annotation.type_annotation,
                    scoping,
                    &ctx.namespace,
                    ctx.imports
                        .is_none_or(|source| !source.requires_registered_global_types()),
                )
            });
            match unknown_name {
                // An unknown name is reported by the whole-file pass in
                // unresolved_names.rs, at the name itself and once; saying it here as
                // well would report it twice. It only decides that this is not a
                // "could not be resolved" warning.
                Some(_) => {}
                None => ctx.warning(
                    crate::diagnostic_messages::messages::unresolvable_type_annotation(&id.name),
                    declarator.span(),
                ),
            }
        }

        (AnnotationOutcome::Resolved(declared), Some(actual)) => {
            note_var_redeclaration(decl_kind, id, declared, ctx);
            if !ctx.is_assignable_instantiating(actual, declared) {
                let instantiated = ctx.instantiate_for_message(actual, declared);
                let whole = crate::diagnostic_messages::messages::declared_type_mismatch_via(
                    &ctx.arena,
                    actual,
                    instantiated,
                    declared,
                );
                report_mismatch(
                    declarator.init.as_ref(),
                    actual,
                    declared,
                    whole,
                    declarator
                        .init
                        .as_ref()
                        .map_or(declarator.span(), GetSpan::span),
                    ctx,
                );
            } else if let Some(init) = &declarator.init {
                check_excess_properties(init, declared, ctx);
            }

            if let Some(symbol_id) = id.symbol_id.get() {
                declare_variable(symbol_id, &id.name, declared, scoping, ctx);
                // Problem: `let x: string | null = "a"` left x as `string | null`, so a
                // read of `x.length` on the next line was reported, though tsc knows x
                // holds the string it was just given.
                // Picked: the initializer narrows a declared union the same way a later
                // assignment does (narrow_on_assignment), and only when the value is
                // assignable, so a mismatch is reported once and narrows nothing.
                // Cost: a declared type that is not a union is left alone, since there
                // is nothing to narrow.
                if matches!(ctx.arena.get(declared), crate::types::Type::Union(_))
                    && ctx.is_assignable_instantiating(actual, declared)
                {
                    ctx.shadow_note_assignment(
                        declarator.node_id.get().index() as u32,
                        declared,
                        actual,
                    );
                    narrow_on_assignment(symbol_id, declared, actual, ctx);
                }
            }
        }

        (AnnotationOutcome::Absent, Some(actual)) => {
            // const x = "a" keeps the literal type "a", since a const binding can
            // never be reassigned to a different string. let x = "a" widens to
            // string, since a later `x = "b"` would otherwise be rejected by a
            // type it was never meant to be pinned to.
            let registered_type = if decl_kind == VariableDeclarationKind::Const {
                actual
            } else {
                crate::types::widen(&ctx.arena, actual)
            };
            let registered_type = evolve_empty_array(registered_type, ctx);
            if let Some(symbol_id) = id.symbol_id.get() {
                declare_variable(symbol_id, &id.name, registered_type, scoping, ctx);
            }
        }

        (AnnotationOutcome::Resolved(declared), None) => {
            note_var_redeclaration(decl_kind, id, declared, ctx);
            if let Some(symbol_id) = id.symbol_id.get() {
                declare_variable(symbol_id, &id.name, declared, scoping, ctx);
            }
        }
        (AnnotationOutcome::Absent, None) => {
            // A variable with no type and no value, in a declaration or exported, has no
            // flow to infer a type from: TS7005.
            if decl_kind != VariableDeclarationKind::Const
                && (decl.declare || ctx.in_export_declaration)
            {
                ctx.error(
                    crate::diagnostic_messages::messages::variable_implicitly_any(&id.name),
                    id.span,
                );
            }
        }
    }
}

// `var i: number; var i: string;` — later declarations of a `var` must repeat the first
// one's type (TS2403). Only annotated declarations are compared, and by mutual assignability,
// so a difference tsc would call identical is never reported.
fn note_var_redeclaration(
    decl_kind: VariableDeclarationKind,
    id: &oxc_ast::ast::BindingIdentifier,
    declared: TypeId,
    ctx: &mut CheckContext<'_, '_>,
) {
    if decl_kind != VariableDeclarationKind::Var {
        return;
    }
    let Some(symbol_id) = id.symbol_id.get() else {
        return;
    };
    let Some(&first) = ctx.var_annotations.get(&symbol_id) else {
        ctx.var_annotations.insert(symbol_id, declared);
        return;
    };
    if first == declared
        || (ctx.is_assignable_instantiating(declared, first)
            && ctx.is_assignable_instantiating(first, declared))
    {
        return;
    }
    let previous = crate::type_display::display_type(&ctx.arena, first);
    let current = crate::type_display::display_type(&ctx.arena, declared);
    ctx.error(
        crate::diagnostic_messages::messages::subsequent_variable_type(
            &id.name, &previous, &current,
        ),
        id.span,
    );
}

// Handles const {x} = y and let [a, b] = y separately from the plain identifier
// case above, since a destructured declarator has no single name to report a
// mismatch against; each nested property or element is checked individually by
// bind_pattern once the overall source_type is known. This function's only job
// is resolving that one source_type.
fn check_destructured_declarator(
    decl_kind: VariableDeclarationKind,
    declarator: &VariableDeclarator,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) {
    let annotation_outcome = match &declarator.type_annotation {
        None => AnnotationOutcome::Absent,
        Some(annotation) => {
            match resolve_type_annotation(annotation, &mut ctx.namespace, &mut ctx.arena) {
                Some(type_id) => AnnotationOutcome::Resolved(type_id),
                None => AnnotationOutcome::Unresolvable,
            }
        }
    };

    // `const [a, b] = [1, "x"]`: tsc reads an array literal that is destructured by an array
    // pattern as a tuple, so each name gets its own element and none is `| undefined`.
    let literal_tuple = match (&declarator.id, &declarator.init) {
        (BindingPattern::ArrayPattern(_), Some(Expression::ArrayExpression(array)))
            if matches!(annotation_outcome, AnnotationOutcome::Absent)
                && !array.elements.is_empty()
                && array.elements.iter().all(|e| {
                    e.as_expression().is_some()
                        && !matches!(e, oxc_ast::ast::ArrayExpressionElement::SpreadElement(_))
                }) =>
        {
            let mut elements = Vec::with_capacity(array.elements.len());
            for element in &array.elements {
                let expr = element.as_expression().expect("checked above");
                ctx.contextual = None;
                let type_id = infer_expression_type(expr, scoping, ctx);
                elements.push(crate::types::TupleElement {
                    type_id: crate::types::widen(&ctx.arena, type_id),
                    optional: false,
                    rest: false,
                });
            }
            Some(ctx.arena.alloc_tuple(elements))
        }
        _ => None,
    };

    let inferred_type = literal_tuple.or_else(|| {
        declarator
            .init
            .as_ref()
            .map(|init| infer_expression_type(init, scoping, ctx))
    });

    let source_type = match (annotation_outcome, inferred_type) {
        (AnnotationOutcome::Unresolvable, _) => {
            let unknown_name = declarator.type_annotation.as_ref().and_then(|annotation| {
                find_unresolved_type_name(
                    &annotation.type_annotation,
                    scoping,
                    &ctx.namespace,
                    ctx.imports
                        .is_none_or(|source| !source.requires_registered_global_types()),
                )
            });
            match unknown_name {
                // An unknown name is reported by the whole-file pass in
                // unresolved_names.rs, at the name itself and once; saying it here as
                // well would report it twice. It only decides that this is not a
                // "could not be resolved" warning.
                Some(_) => {}
                None => ctx.warning(
                    crate::diagnostic_messages::messages::unresolvable_destructuring_type_annotation(),
                    declarator.span(),
                ),
            }
            return;
        }
        (AnnotationOutcome::Resolved(declared), Some(actual)) => {
            if !ctx.is_assignable_instantiating(actual, declared) {
                let whole =
                    crate::diagnostic_messages::messages::destructuring_pattern_type_mismatch(
                        &ctx.arena, actual, declared,
                    );
                report_mismatch(
                    declarator.init.as_ref(),
                    actual,
                    declared,
                    whole,
                    declarator
                        .init
                        .as_ref()
                        .map_or(declarator.span(), GetSpan::span),
                    ctx,
                );
            }
            declared
        }
        (AnnotationOutcome::Resolved(declared), None) => declared,
        (AnnotationOutcome::Absent, Some(actual)) => {
            let widened = if decl_kind == VariableDeclarationKind::Const {
                actual
            } else {
                crate::types::widen(&ctx.arena, actual)
            };
            evolve_empty_array(widened, ctx)
        }
        (AnnotationOutcome::Absent, None) => return,
    };

    bind_pattern(&declarator.id, source_type, scoping, ctx);
    if decl_kind == VariableDeclarationKind::Const
        && let BindingPattern::ObjectPattern(object) = &declarator.id
    {
        crate::bridge::narrow::register_destructured_group(object, source_type, ctx);
    }
}

// `const xs = []` followed by `xs.push(1)` is ordinary code, and tsc accepts it by letting
// the array's element type grow as it is assigned to. This checker has no growing types, so
// an unannotated variable initialised with an empty array literal (typed never[], which
// nothing can be pushed into) is given any[] instead, the permissive reading. An annotated
// variable keeps exactly what it was annotated with, and an empty array nested in an
// object literal stays never[], as it does in tsc.
fn evolve_empty_array(type_id: TypeId, ctx: &mut CheckContext<'_, '_>) -> TypeId {
    let is_empty_array = matches!(
        ctx.arena.get(type_id),
        Type::Array(element) if *element == ctx.arena.never()
    );
    if is_empty_array {
        let any = ctx.arena.any();
        ctx.arena.alloc(Type::Array(any))
    } else {
        type_id
    }
}

// Declares a variable's type; one declared at the top of the file can also be named by
// `typeof` in an annotation.
fn declare_variable(
    symbol_id: oxc_semantic::SymbolId,
    name: &str,
    type_id: TypeId,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) {
    if scoping.symbol_scope_id(symbol_id) == scoping.root_scope_id() {
        ctx.declare_value(symbol_id, name, type_id);
    } else {
        ctx.symbols.declare(symbol_id, type_id);
    }
}
