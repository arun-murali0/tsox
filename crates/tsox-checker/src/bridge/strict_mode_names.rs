use oxc_ast::ast::{
    AssignmentExpression, AssignmentTarget, BindingPattern, CatchClause, Class, Function,
    FunctionType, Program, SimpleAssignmentTarget, UpdateExpression, VariableDeclarator,
};
use oxc_ast_visit::{Visit, walk};
use oxc_span::Span;

use crate::diagnostic_messages::messages;

use super::context::CheckContext;
use super::exports::is_module_file;

// Problem: `var arguments`, `function f(eval)` and `arguments = 1` are errors wherever the code
// is strict, and every file is once `moduleDetection` is forced or `alwaysStrict` is on; tsox
// said nothing (TS1215 in a module, TS1210 inside a class, TS1100 otherwise).
// Picked: the places tsc's binder checks: a variable or parameter or catch binding (also inside
// a destructuring pattern), the name of a function declaration or expression, the target of an
// assignment (compound included), and the operand of `++` and `--`. Strict means the file is a
// module, `alwaysStrict` is on, a class encloses the code, or the file opens with "use strict".
// Cost: a function body with its own "use strict" directive is not noticed.
pub fn report_strict_mode_names<'ast>(
    program: &'ast Program<'ast>,
    ctx: &mut CheckContext<'ast, '_>,
) {
    let in_module = is_module_file(program, &ctx.options);
    let file_strict = ctx.options.always_strict
        || in_module
        || program
            .directives
            .iter()
            .any(|directive| directive.directive == "use strict");
    let mut walker = Walker {
        ctx,
        file_strict,
        in_module,
        class_depth: 0,
    };
    walker.visit_program(program);
}

struct Walker<'c, 'ast, 'src> {
    ctx: &'c mut CheckContext<'ast, 'src>,
    file_strict: bool,
    in_module: bool,
    class_depth: u32,
}

impl Walker<'_, '_, '_> {
    fn check(&mut self, name: &str, span: Span) {
        if !matches!(name, "eval" | "arguments") || !(self.file_strict || self.class_depth > 0) {
            return;
        }
        let message =
            messages::eval_or_arguments_in_strict_mode(name, self.class_depth > 0, self.in_module);
        let already = self
            .ctx
            .diagnostics
            .iter()
            .any(|d| d.start == span.start && d.code == message.code);
        if !already {
            self.ctx.error(message, span);
        }
    }

    fn check_pattern(&mut self, pattern: &BindingPattern<'_>) {
        match pattern {
            BindingPattern::BindingIdentifier(id) => self.check(&id.name, id.span),
            BindingPattern::ObjectPattern(object) => {
                for property in &object.properties {
                    self.check_pattern(&property.value);
                }
                if let Some(rest) = &object.rest {
                    self.check_pattern(&rest.argument);
                }
            }
            BindingPattern::ArrayPattern(array) => {
                for element in array.elements.iter().flatten() {
                    self.check_pattern(element);
                }
                if let Some(rest) = &array.rest {
                    self.check_pattern(&rest.argument);
                }
            }
            BindingPattern::AssignmentPattern(assignment) => self.check_pattern(&assignment.left),
        }
    }
}

impl<'a> Visit<'a> for Walker<'_, '_, '_> {
    fn visit_class(&mut self, it: &Class<'a>) {
        self.class_depth += 1;
        walk::walk_class(self, it);
        self.class_depth -= 1;
    }

    fn visit_variable_declarator(&mut self, it: &VariableDeclarator<'a>) {
        self.check_pattern(&it.id);
        walk::walk_variable_declarator(self, it);
    }

    fn visit_function(&mut self, it: &Function<'a>, flags: oxc_semantic::ScopeFlags) {
        if matches!(
            it.r#type,
            FunctionType::FunctionDeclaration | FunctionType::FunctionExpression
        ) && let Some(id) = &it.id
        {
            self.check(&id.name, it.span);
        }
        for parameter in &it.params.items {
            self.check_pattern(&parameter.pattern);
        }
        if let Some(rest) = &it.params.rest {
            self.check_pattern(&rest.rest.argument);
        }
        walk::walk_function(self, it, flags);
    }

    fn visit_catch_clause(&mut self, it: &CatchClause<'a>) {
        if let Some(parameter) = &it.param {
            self.check_pattern(&parameter.pattern);
        }
        walk::walk_catch_clause(self, it);
    }

    fn visit_assignment_expression(&mut self, it: &AssignmentExpression<'a>) {
        if let AssignmentTarget::AssignmentTargetIdentifier(id) = &it.left {
            self.check(&id.name, it.span);
        }
        walk::walk_assignment_expression(self, it);
    }

    fn visit_update_expression(&mut self, it: &UpdateExpression<'a>) {
        if let SimpleAssignmentTarget::AssignmentTargetIdentifier(id) = &it.argument {
            self.check(&id.name, it.span);
        }
        walk::walk_update_expression(self, it);
    }
}
