//! Calls with a fixed meaning that never reach overload resolution. Each form looks at the call
//! and answers or declines; `CALL_FORMS` is tried in order and the first answer wins, so a new
//! form is one function and one table row.
use super::*;

pub(super) type CallForm = for<'a, 'b, 'c> fn(
    &oxc_ast::ast::CallExpression<'a>,
    &Scoping,
    &mut CheckContext<'b, 'c>,
) -> Option<TypeId>;

#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct Form {
    pub name: &'static str,
    pub doc: &'static str,
    pub run: CallForm,
}

pub(super) const CALL_FORMS: &[Form] = &[
    Form {
        name: "global_conversion",
        doc: "`String(x)`, `Number(x)`, `Boolean(x)` convert: the result is the primitive.",
        run: global_conversion,
    },
    Form {
        name: "promise_static",
        doc: "`Promise.resolve/reject/all/race/...` on the global Promise.",
        run: promise_static,
    },
];

pub(super) fn special_call_form(
    call: &oxc_ast::ast::CallExpression<'_>,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> Option<TypeId> {
    CALL_FORMS
        .iter()
        .find_map(|form| (form.run)(call, scoping, ctx))
}

fn global_conversion(
    call: &oxc_ast::ast::CallExpression<'_>,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> Option<TypeId> {
    let Expression::Identifier(ident) = &call.callee else {
        return None;
    };
    if resolve_symbol_id(ident, scoping).is_some() {
        return None;
    }
    let primitive = match ident.name.as_str() {
        "String" => ctx.arena.string(),
        "Number" => ctx.arena.number(),
        "Boolean" => ctx.arena.boolean(),
        _ => return None,
    };
    for argument in &call.arguments {
        if let Some(expression) = argument.as_expression() {
            infer_expression_type(expression, scoping, ctx);
        }
    }
    Some(primitive)
}

fn promise_static(
    call: &oxc_ast::ast::CallExpression<'_>,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> Option<TypeId> {
    let Expression::StaticMemberExpression(member) = &call.callee else {
        return None;
    };
    promise_static_call(call, member, scoping, ctx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forms_have_unique_names_and_docs() {
        let mut seen = std::collections::HashSet::new();
        for form in CALL_FORMS {
            assert!(seen.insert(form.name), "duplicate form {}", form.name);
            assert!(!form.doc.is_empty());
        }
    }
}
