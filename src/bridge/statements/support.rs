use oxc_ast::ast::{
    BindingPattern, FormalParameters, Statement, TSSignature, TSTupleElement, TSType, TSTypeName,
};
use oxc_semantic::Scoping;
use oxc_span::{GetSpan, Span};

use crate::namespace::TypeNamespace;

use super::super::context::CheckContext;
use super::super::unresolved_names::is_unresolved_type_name;

pub(super) fn push_unsupported(stmt: &Statement, ctx: &mut CheckContext<'_, '_>) {
    let kind = stmt_kind_name(stmt);
    tracing::trace!(kind, "unsupported statement kind");
    ctx.warning(
        crate::diagnostic_messages::messages::unimplemented_statement_kind(kind),
        stmt.span(),
    );
}

fn stmt_kind_name(stmt: &Statement) -> &'static str {
    match stmt {
        Statement::ImportDeclaration(_) => "ImportDeclaration",
        _ => "Other",
    }
}

// Used by control_flow.rs to decide whether an if branch's narrowing should
// survive past the whole if statement. `if (x === null) return; ... x.prop`
// depends on this: since the consequent branch always exits, only the else
// branch's narrowing (x is non-null) can reach code after the if, and that
// narrowing should carry forward rather than be discarded at the closing brace.
pub(crate) fn statement_always_exits(stmt: &Statement) -> bool {
    match stmt {
        Statement::ReturnStatement(_) | Statement::ThrowStatement(_) => true,
        Statement::BlockStatement(block) => block.body.last().is_some_and(statement_always_exits),
        Statement::IfStatement(if_stmt) => match &if_stmt.alternate {
            Some(alternate) => {
                statement_always_exits(&if_stmt.consequent) && statement_always_exits(alternate)
            }
            None => false,
        },
        _ => false,
    }
}

// Whether nothing after `stmt` in the same statement list can run, because it returns,
// throws, breaks or continues. statement_always_exits above answers the same question
// for the unreachable-code report and is left as it was; this one also counts `break`
// and `continue` and sees through `try`, which is what narrowing needs: a branch that
// ends in `break` hands its state to the loop's exit, and contributes nothing to the
// statement after the `if`.
pub(crate) fn statement_leaves_flow(stmt: &Statement) -> bool {
    match stmt {
        Statement::ReturnStatement(_)
        | Statement::ThrowStatement(_)
        | Statement::BreakStatement(_)
        | Statement::ContinueStatement(_) => true,
        Statement::BlockStatement(block) => block.body.last().is_some_and(statement_leaves_flow),
        Statement::IfStatement(if_stmt) => match &if_stmt.alternate {
            Some(alternate) => {
                statement_leaves_flow(&if_stmt.consequent) && statement_leaves_flow(alternate)
            }
            None => false,
        },
        Statement::TryStatement(try_stmt) => {
            let finalizer_leaves = try_stmt
                .finalizer
                .as_ref()
                .is_some_and(|finalizer| finalizer.body.last().is_some_and(statement_leaves_flow));
            let block_leaves = try_stmt
                .block
                .body
                .last()
                .is_some_and(statement_leaves_flow);
            let handler_leaves = try_stmt
                .handler
                .as_ref()
                .is_none_or(|handler| handler.body.body.last().is_some_and(statement_leaves_flow));
            finalizer_leaves || (block_leaves && handler_leaves)
        }
        _ => false,
    }
}

// Whether a `break` appears anywhere inside `stmt`. Deliberately generous: a break
// that belongs to a nested loop or switch is counted too, and a `try` or `with` is
// assumed to hide one, because the only use of this answer is to decide that a loop
// can only be left by its test failing, and a false "yes there is a break" merely
// skips a narrowing. A function body is not entered: a break cannot cross it.
pub(crate) fn contains_break(stmt: &Statement) -> bool {
    match stmt {
        Statement::BreakStatement(_) => true,
        Statement::BlockStatement(block) => block.body.iter().any(contains_break),
        Statement::IfStatement(if_stmt) => {
            contains_break(&if_stmt.consequent)
                || if_stmt.alternate.as_ref().is_some_and(contains_break)
        }
        Statement::WhileStatement(inner) => contains_break(&inner.body),
        Statement::DoWhileStatement(inner) => contains_break(&inner.body),
        Statement::ForStatement(inner) => contains_break(&inner.body),
        Statement::ForInStatement(inner) => contains_break(&inner.body),
        Statement::ForOfStatement(inner) => contains_break(&inner.body),
        Statement::LabeledStatement(inner) => contains_break(&inner.body),
        Statement::SwitchStatement(inner) => inner
            .cases
            .iter()
            .any(|case| case.consequent.iter().any(contains_break)),
        Statement::TryStatement(_) | Statement::WithStatement(_) => true,
        _ => false,
    }
}

// Reports every parameter with no type annotation and no default value as an
// implicit `any`, the same condition tsc reports under noImplicitAny. Callers
// decide *where* this applies: it is only called for parameter lists that can
// never receive a contextual type (function declarations, class constructors
// and methods, and arrow or function expressions used as the initializer of an
// unannotated variable). An arrow passed as a call argument is deliberately not
// reported, since tsc would type its parameters from the callee's signature and
// this checker does not do contextual typing yet.
//
// A destructured parameter, a parameter with a default (`x = 1`), and a rest
// parameter are skipped: tsc infers from the default, and reports the other two
// under different diagnostics.
pub(super) fn report_implicit_any_params(
    params: &FormalParameters,
    ctx: &mut CheckContext<'_, '_>,
) {
    for param in &params.items {
        // Problem: the comment above promised that a default skips the report, but only the
        // annotation was tested, so `y = false` and `foo = {}` read as implicit any, though
        // tsc types them from the default.
        if param.type_annotation.is_some() || param.initializer.is_some() {
            continue;
        }
        let BindingPattern::BindingIdentifier(id) = &param.pattern else {
            continue;
        };
        ctx.error(
            crate::diagnostic_messages::messages::parameter_implicitly_any(&id.name),
            id.span,
        );
    }
}

// The type written inside a tuple element: `A`, `A?`, `...A[]` and `name: A` all hold one.
fn tuple_element_type<'b, 'a>(element: &'b TSTupleElement<'a>) -> Option<&'b TSType<'a>> {
    match element {
        TSTupleElement::TSOptionalType(optional) => Some(&optional.type_annotation),
        TSTupleElement::TSRestType(rest) => Some(&rest.type_annotation),
        other => match other.as_ts_type()? {
            TSType::TSNamedTupleMember(member) => tuple_element_type(&member.element_type),
            ty => Some(ty),
        },
    }
}

// Finds the first type name in an annotation that genuinely does not exist, as
// opposed to one that exists but this checker cannot represent. A name counts as
// missing only if oxc's own scope analysis resolved it to no declaration at all
// (which also correctly covers imports, exported declarations and type
// parameters that have gone out of scope), it is not in this checker's own
// namespace, and it is not a lib.d.ts global. Anything else that fails to
// resolve is an unsupported construct, and stays a warning.
pub(super) fn find_unresolved_type_name(
    ty: &TSType,
    scoping: &Scoping,
    namespace: &TypeNamespace,
) -> Option<(String, Span)> {
    match ty {
        TSType::TSTypeReference(reference) => {
            let TSTypeName::IdentifierReference(id) = &reference.type_name else {
                return None;
            };
            let is_missing = is_unresolved_type_name(id, scoping, namespace);
            is_missing.then(|| (id.name.to_string(), id.span))
        }
        TSType::TSArrayType(array) => {
            find_unresolved_type_name(&array.element_type, scoping, namespace)
        }
        TSType::TSTupleType(tuple) => tuple.element_types.iter().find_map(|element| {
            let ty = tuple_element_type(element)?;
            find_unresolved_type_name(ty, scoping, namespace)
        }),
        TSType::TSUnionType(union) => union
            .types
            .iter()
            .find_map(|member| find_unresolved_type_name(member, scoping, namespace)),
        TSType::TSIntersectionType(intersection) => intersection
            .types
            .iter()
            .find_map(|member| find_unresolved_type_name(member, scoping, namespace)),
        TSType::TSTypeLiteral(literal) => literal.members.iter().find_map(|member| {
            let TSSignature::TSPropertySignature(property) = member else {
                return None;
            };
            let annotation = property.type_annotation.as_ref()?;
            find_unresolved_type_name(&annotation.type_annotation, scoping, namespace)
        }),
        TSType::TSFunctionType(func) => func
            .params
            .items
            .iter()
            .find_map(|param| {
                let annotation = param.type_annotation.as_ref()?;
                find_unresolved_type_name(&annotation.type_annotation, scoping, namespace)
            })
            .or_else(|| {
                find_unresolved_type_name(&func.return_type.type_annotation, scoping, namespace)
            }),
        _ => None,
    }
}
