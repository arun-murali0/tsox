use oxc_ast::ast::{Class, Program, Statement, TSTypeName};
use oxc_span::GetSpan;

use crate::namespace::Resolution;

use super::context::CheckContext;

// `class C implements I`: the instance type of C must be assignable to I (TS2420). Only a
// class and an interface that both resolve completely are compared, so a shape this checker
// cannot see never produces a report; the cost is that such a class goes unchecked.
pub fn check_class_implements<'ast>(program: &Program<'ast>, ctx: &mut CheckContext<'ast, '_>) {
    for statement in &program.body {
        match statement {
            Statement::ClassDeclaration(class) => check_class(class, ctx),
            Statement::ExportDeclaration(export) => {
                if let oxc_ast::ast::Declaration::ClassDeclaration(class) = &export.declaration {
                    check_class(class, ctx);
                }
            }
            _ => {}
        }
    }
}

fn check_class<'ast>(class: &Class<'ast>, ctx: &mut CheckContext<'ast, '_>) {
    let Some(id) = class.id.as_ref() else {
        return;
    };
    if class.implements.is_empty() {
        return;
    }
    let Resolution::Resolved(instance) = ctx.namespace.resolve(&id.name, &mut ctx.arena) else {
        return;
    };
    for implemented in &class.implements {
        let TSTypeName::IdentifierReference(name) = &implemented.expression else {
            continue;
        };
        let Some(interface) = crate::type_annotation::resolve_named_reference(
            &name.name,
            implemented.type_arguments.as_deref(),
            implemented.span(),
            &mut ctx.namespace,
            &mut ctx.arena,
        ) else {
            continue;
        };
        if crate::subtyping::is_subtype(&ctx.arena, instance, interface) {
            continue;
        }
        match first_incompatible_property(ctx, instance, interface) {
            // A property both sides have, with types that disagree: tsc reports it at the
            // class member (TS2416). Without a member to point at, the class takes it.
            Some((property, true)) => {
                let span = member_span(class, &property).unwrap_or_else(|| implemented.span());
                ctx.error(
                    crate::diagnostic_messages::messages::class_property_incompatible(
                        &ctx.arena, &property, &id.name, interface,
                    ),
                    span,
                );
            }
            _ => ctx.error(
                crate::diagnostic_messages::messages::class_incorrectly_implements(
                    &ctx.arena, &id.name, interface,
                ),
                implemented.span(),
            ),
        }
    }
}

/// The first property of `interface` the instance lacks (`false`) or has with an
/// incompatible type (`true`).
fn first_incompatible_property(
    ctx: &CheckContext<'_, '_>,
    instance: crate::arena::TypeId,
    interface: crate::arena::TypeId,
) -> Option<(String, bool)> {
    use crate::types::Type;
    let (Type::Object(have), Type::Object(want)) =
        (ctx.arena.get(instance), ctx.arena.get(interface))
    else {
        return None;
    };
    for property in want.properties.iter() {
        match have.properties.iter().find(|p| p.name == property.name) {
            None if !property.optional => return Some((property.name.to_string(), false)),
            None => {}
            Some(own) => {
                if !crate::subtyping::is_subtype(&ctx.arena, own.type_id, property.type_id) {
                    return Some((property.name.to_string(), true));
                }
            }
        }
    }
    None
}

fn member_span(class: &Class<'_>, name: &str) -> Option<oxc_span::Span> {
    use oxc_ast::ast::ClassElement;
    class.body.body.iter().find_map(|element| match element {
        ClassElement::PropertyDefinition(p) if p.key.static_name().as_deref() == Some(name) => {
            Some(p.key.span())
        }
        ClassElement::MethodDefinition(m) if m.key.static_name().as_deref() == Some(name) => {
            Some(m.key.span())
        }
        _ => None,
    })
}
