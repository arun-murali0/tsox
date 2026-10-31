use oxc_ast::ast::{Program, TSTupleElement, TSTupleType, TSType};
use oxc_ast_visit::{Visit, walk};
use oxc_span::GetSpan;

use crate::types::Type;

use super::context::CheckContext;

// tsc's three rules about the order of a tuple's elements, reported at the element that
// breaks them, as tsc's own checker does (checkTupleType):
//   - a required element cannot follow an optional one (TS1257),
//   - a rest element cannot follow another rest element (TS1265),
//   - an optional element cannot follow a rest element (TS1266).
// Like tsc, a tuple reports its first broken rule and no more: one misplaced `?` is one error,
// however many elements follow it.
// A required element after a rest element is fine (`[...string[], number]`), and so is
// an optional one after a required one.
// The parser notices the same three things and calls them syntax errors; parse.rs lets a
// file with only those through, and this pass is where they become diagnostics. It reads the
// written tuple and not the resolved type, so it still reports a tuple whose elements
// do not resolve.
pub(crate) fn report_tuple_grammar_errors<'ast>(
    program: &Program<'ast>,
    ctx: &mut CheckContext<'ast, '_>,
) {
    let mut walker = Walker { ctx };
    walker.visit_program(program);
}

struct Walker<'c, 'ast, 'src> {
    ctx: &'c mut CheckContext<'ast, 'src>,
}

// tsc tells two kinds of `...` apart, and only one of them is a "rest element" for these rules:
//   - a rest element is `...X[]` (or `...readonly X[]`, possibly in parentheses): an array
//     written out. Two of them, or an optional element after one, is an error.
//   - a variadic element is any other `...T`: a type parameter, `infer R`, or a named type.
//     It is never an error itself (`[...T, ...U]` and `[...A, ...infer R]` are fine, and so
//     is a spread of a tuple that has no rest), but when it turns out to be an array, or a
//     tuple with a rest element, a later rest element or optional element is.
// A variadic element that does not resolve is taken as not being an array, so a type this
// checker cannot read never produces an error tsc would not.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Required,
    Optional,
    Rest,
    /// `...T`; true when it is known to be an array or a tuple with a rest element.
    Variadic(bool),
}

fn is_written_array(ty: &TSType) -> bool {
    match ty {
        TSType::TSArrayType(_) => true,
        TSType::TSParenthesizedType(inner) => is_written_array(&inner.type_annotation),
        TSType::TSTypeOperatorType(operator) => is_written_array(&operator.type_annotation),
        _ => false,
    }
}

fn spread_is_array_like(ty: &TSType, ctx: &mut CheckContext<'_, '_>) -> bool {
    if let TSType::TSTupleType(tuple) = ty {
        return tuple
            .element_types
            .iter()
            .any(|element| matches!(kind_of(element, ctx), Kind::Rest | Kind::Variadic(true)));
    }
    let Some(resolved) =
        crate::type_annotation::resolve_ts_type(ty, &mut ctx.namespace, &mut ctx.arena)
    else {
        return false;
    };
    match ctx.arena.get(resolved) {
        Type::Array(_) => true,
        Type::Tuple(elements) => elements.iter().any(|e| e.rest),
        _ => false,
    }
}

fn kind_of_spread(ty: &TSType, ctx: &mut CheckContext<'_, '_>) -> Kind {
    if is_written_array(ty) {
        Kind::Rest
    } else {
        Kind::Variadic(spread_is_array_like(ty, ctx))
    }
}

fn kind_of(element: &TSTupleElement, ctx: &mut CheckContext<'_, '_>) -> Kind {
    match element {
        TSTupleElement::TSOptionalType(_) => Kind::Optional,
        TSTupleElement::TSRestType(rest) => kind_of_spread(&rest.type_annotation, ctx),
        other => match other.as_ts_type() {
            // `[name: T]`, `[name?: T]`, and `[...name: T[]]` when the parser keeps the
            // rest inside the label.
            Some(TSType::TSNamedTupleMember(member)) => {
                if member.optional {
                    Kind::Optional
                } else if let TSTupleElement::TSRestType(rest) = &member.element_type {
                    kind_of_spread(&rest.type_annotation, ctx)
                } else {
                    Kind::Required
                }
            }
            _ => Kind::Required,
        },
    }
}

impl<'a> Visit<'a> for Walker<'_, '_, '_> {
    fn visit_ts_tuple_type(&mut self, it: &TSTupleType<'a>) {
        let mut seen_rest = false;
        let mut seen_optional = false;
        for element in &it.element_types {
            match kind_of(element, self.ctx) {
                Kind::Rest => {
                    if seen_rest {
                        self.ctx.error(
                            crate::diagnostic_messages::messages::tuple_rest_after_rest(),
                            element.span(),
                        );
                        break;
                    }
                    seen_rest = true;
                }
                // Never an error itself; it only counts as a rest once it is known to be one.
                Kind::Variadic(array_like) => seen_rest |= array_like,
                Kind::Optional => {
                    if seen_rest {
                        self.ctx.error(
                            crate::diagnostic_messages::messages::tuple_optional_after_rest(),
                            element.span(),
                        );
                        break;
                    }
                    seen_optional = true;
                }
                Kind::Required => {
                    if seen_optional {
                        self.ctx.error(
                            crate::diagnostic_messages::messages::tuple_required_after_optional(),
                            element.span(),
                        );
                        break;
                    }
                }
            }
        }
        walk::walk_ts_tuple_type(self, it);
    }
}
