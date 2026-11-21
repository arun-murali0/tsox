use oxc_ast::ast::{Function, Program};
use oxc_ast_visit::{Visit, walk};
use oxc_span::GetSpan;

use crate::diagnostic_messages::messages;

use super::context::CheckContext;

// Problem: `function f(public x) {}` and a method `m(private x)` declare a parameter property,
// which only a constructor with a body can; tsox said nothing (TS2369).
// Picked: every parameter carrying `public`, `private`, `protected`, `readonly` or `override`
// is reported unless its function is a class constructor that has a body. Constructor
// overloads and an `abstract` or ambient constructor have no body and are reported too, as in
// tsc.
pub fn report_parameter_properties<'ast>(
    program: &'ast Program<'ast>,
    ctx: &mut CheckContext<'ast, '_>,
) {
    let mut walker = Walker {
        ctx,
        constructor_with_body: false,
    };
    walker.visit_program(program);
}

struct Walker<'c, 'ast, 'src> {
    ctx: &'c mut CheckContext<'ast, 'src>,
    // Set while visiting the parameters of the function being walked: whether it is a
    // constructor implementation.
    constructor_with_body: bool,
}

impl<'ast> Visit<'ast> for Walker<'_, 'ast, '_> {
    fn visit_method_definition(&mut self, method: &oxc_ast::ast::MethodDefinition<'ast>) {
        let is_constructor = matches!(method.kind, oxc_ast::ast::MethodDefinitionKind::Constructor);
        let saved = self.constructor_with_body;
        self.constructor_with_body = is_constructor && method.value.body.is_some();
        walk::walk_method_definition(self, method);
        self.constructor_with_body = saved;
    }

    fn visit_function(&mut self, function: &Function<'ast>, flags: oxc_semantic::ScopeFlags) {
        // A function that is not the value of a constructor is never an implementation of one.
        // `visit_method_definition` set the flag for its own value; anything nested starts over.
        let allowed = self.constructor_with_body;
        self.constructor_with_body = false;
        for parameter in &function.params.items {
            let is_property =
                parameter.accessibility.is_some() || parameter.readonly || parameter.r#override;
            if is_property && !allowed {
                self.ctx.error(
                    messages::parameter_property_outside_constructor(),
                    parameter.span(),
                );
            }
        }
        walk::walk_function(self, function, flags);
        self.constructor_with_body = false;
    }
}
