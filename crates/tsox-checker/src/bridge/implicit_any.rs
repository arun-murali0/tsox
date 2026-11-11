// The implicit-`any` reports that depend on the declaration alone and never on a
// contextual type: a member with no type and no initializer (TS7008), a function, method
// or signature with no body and no return type (TS7010, TS7020, TS7013), and the
// parameters of such declarations, of setters and of function types (TS7006).
//
// A parameter of a function that has a body is reported where the body is checked, so
// this pass leaves those alone and a diagnostic is never written twice.

use oxc_ast::ast::{
    BindingPattern, FormalParameters, Function, MethodDefinitionKind, Program, PropertyKey,
    TSCallSignatureDeclaration, TSConstructSignatureDeclaration, TSConstructorType, TSFunctionType,
    TSMethodSignature, TSMethodSignatureKind, TSPropertySignature,
};
use oxc_ast::ast::{MethodDefinition, PropertyDefinition};
use oxc_ast_visit::{Visit, walk};
use oxc_span::{GetSpan, Span};

use crate::diagnostic_codes::DiagnosticCode;
use crate::diagnostic_messages::messages;

use super::context::CheckContext;

pub fn report_implicit_any<'ast>(program: &Program<'ast>, ctx: &mut CheckContext<'ast, '_>) {
    let mut walker = Walker {
        ctx,
        source: program.source_text,
        ambient: 0,
        constructor_assigned: Vec::new(),
    };
    walker.visit_program(program);
}

struct Walker<'c, 'ast, 'src> {
    ctx: &'c mut CheckContext<'ast, 'src>,
    source: &'c str,
    // Inside `declare class`, `declare namespace` and the like. Their private members
    // carry no types in a declaration, so tsc asks none of them.
    ambient: u32,
    // Per enclosing class: the members its constructor assigns through `this`. tsc infers
    // an untyped member's type from those, so such a member is not an implicit any.
    constructor_assigned: Vec<std::collections::HashSet<String>>,
}

// How tsc writes a member name in these messages: an identifier as is, a string with its
// quotes, a number as written. A computed key has no name to report.
fn key_name(key: &PropertyKey<'_>, source: &str) -> Option<String> {
    match key {
        PropertyKey::StaticIdentifier(id) => Some(id.name.to_string()),
        PropertyKey::PrivateIdentifier(id) => Some(format!("#{}", id.name)),
        PropertyKey::StringLiteral(lit) => {
            let span = lit.span;
            source
                .get(span.start as usize..span.end as usize)
                .map(str::to_string)
        }
        PropertyKey::NumericLiteral(lit) => {
            let span = lit.span;
            source
                .get(span.start as usize..span.end as usize)
                .map(str::to_string)
        }
        _ => None,
    }
}

impl<'c, 'ast, 'src> Walker<'c, 'ast, 'src> {
    fn already(&self, code: DiagnosticCode, start: u32) -> bool {
        self.ctx
            .diagnostics
            .iter()
            .any(|d| d.code == code && d.start == start)
    }

    fn report(
        &mut self,
        code: DiagnosticCode,
        message: crate::diagnostic_messages::DiagnosticMessage,
        span: Span,
    ) {
        if !self.already(code, span.start) {
            self.ctx.error(message, span);
        }
    }

    fn params(&mut self, params: &FormalParameters<'_>) {
        for param in &params.items {
            if param.type_annotation.is_some() || param.initializer.is_some() {
                continue;
            }
            let BindingPattern::BindingIdentifier(id) = &param.pattern else {
                let mut found = Vec::new();
                super::statements::implicit_any_bindings(&param.pattern, &mut found);
                for (name, span) in found {
                    self.report(
                        DiagnosticCode::ImplicitAnyBinding,
                        messages::binding_element_implicitly_any(&name),
                        span,
                    );
                }
                continue;
            };
            self.report(
                DiagnosticCode::ImplicitAnyParameter,
                messages::parameter_implicitly_any(&id.name),
                id.span,
            );
        }
        if let Some(rest) = &params.rest
            && rest.type_annotation.is_none()
            && let BindingPattern::BindingIdentifier(id) = &rest.rest.argument
        {
            self.report(
                DiagnosticCode::ImplicitAnyParameter,
                messages::rest_parameter_implicitly_any(&id.name),
                id.span,
            );
        }
    }

    fn lacks_return(&mut self, key: &PropertyKey<'_>, computed: bool) {
        if computed {
            return;
        }
        let source = self.source;
        if let Some(name) = key_name(key, source) {
            self.report(
                DiagnosticCode::ImplicitAnyFunctionReturn,
                messages::function_lacks_return_annotation(&name),
                key.span(),
            );
        }
    }
}

impl<'a> Visit<'a> for Walker<'_, '_, '_> {
    fn visit_class(&mut self, class: &oxc_ast::ast::Class<'a>) {
        let ambient = class.declare;
        self.ambient += u32::from(ambient);
        let mut assigned = ThisAssignments::default();
        for element in &class.body.body {
            if let oxc_ast::ast::ClassElement::MethodDefinition(method) = element
                && method.kind == MethodDefinitionKind::Constructor
                && let Some(body) = &method.value.body
            {
                assigned.visit_function_body(body);
            }
        }
        self.constructor_assigned.push(assigned.names);
        walk::walk_class(self, class);
        self.constructor_assigned.pop();
        self.ambient -= u32::from(ambient);
    }

    fn visit_ts_namespace_declaration(&mut self, decl: &oxc_ast::ast::TSNamespaceDeclaration<'a>) {
        self.ambient += u32::from(decl.declare);
        walk::walk_ts_namespace_declaration(self, decl);
        self.ambient -= u32::from(decl.declare);
    }

    fn visit_ts_external_module_declaration(
        &mut self,
        decl: &oxc_ast::ast::TSExternalModuleDeclaration<'a>,
    ) {
        self.ambient += 1;
        walk::walk_ts_external_module_declaration(self, decl);
        self.ambient -= 1;
    }

    fn visit_function(&mut self, func: &Function<'a>, flags: oxc_semantic::ScopeFlags) {
        if func.body.is_none() {
            self.params(&func.params);
            if func.return_type.is_none()
                && let Some(id) = &func.id
            {
                self.report(
                    DiagnosticCode::ImplicitAnyFunctionReturn,
                    messages::function_lacks_return_annotation(&id.name),
                    id.span,
                );
            }
        }
        walk::walk_function(self, func, flags);
    }

    fn visit_method_definition(&mut self, method: &MethodDefinition<'a>) {
        let func = &method.value;
        let exempt = self.ambient > 0
            && method.accessibility == Some(oxc_ast::ast::TSAccessibility::Private);
        if exempt {
            return;
        }
        if func.body.is_none() {
            // The `function` visit that follows reports the parameters of a bodyless
            // method; only the name needs reporting here.
            if method.kind == MethodDefinitionKind::Method && func.return_type.is_none() {
                self.lacks_return(&method.key, method.computed);
            }
        }
        walk::walk_method_definition(self, method);
    }

    fn visit_property_definition(&mut self, property: &PropertyDefinition<'a>) {
        let exempt = self.ambient > 0
            && property.accessibility == Some(oxc_ast::ast::TSAccessibility::Private);
        if !exempt
            && property.type_annotation.is_none()
            && property.value.is_none()
            && !property.computed
        {
            let source = self.source;
            if let Some(name) = key_name(&property.key, source)
                && !self
                    .constructor_assigned
                    .last()
                    .is_some_and(|names| names.contains(name.trim_matches(['\'', '"'])))
            {
                self.report(
                    DiagnosticCode::ImplicitAnyMember,
                    messages::member_implicitly_any(&name),
                    property.key.span(),
                );
            }
        }
        walk::walk_property_definition(self, property);
    }

    fn visit_ts_property_signature(&mut self, signature: &TSPropertySignature<'a>) {
        if signature.type_annotation.is_none() && !signature.computed {
            let source = self.source;
            if let Some(name) = key_name(&signature.key, source) {
                self.report(
                    DiagnosticCode::ImplicitAnyMember,
                    messages::member_implicitly_any(&name),
                    signature.key.span(),
                );
            }
        }
        walk::walk_ts_property_signature(self, signature);
    }

    fn visit_ts_method_signature(&mut self, signature: &TSMethodSignature<'a>) {
        self.params(&signature.params);
        if signature.return_type.is_none() && signature.kind == TSMethodSignatureKind::Method {
            self.lacks_return(&signature.key, signature.computed);
        }
        walk::walk_ts_method_signature(self, signature);
    }

    fn visit_ts_call_signature_declaration(&mut self, signature: &TSCallSignatureDeclaration<'a>) {
        self.params(&signature.params);
        if signature.return_type.is_none() {
            self.report(
                DiagnosticCode::ImplicitAnyCallSignature,
                messages::call_signature_lacks_return_annotation(),
                signature.span,
            );
        }
        walk::walk_ts_call_signature_declaration(self, signature);
    }

    fn visit_ts_construct_signature_declaration(
        &mut self,
        signature: &TSConstructSignatureDeclaration<'a>,
    ) {
        self.params(&signature.params);
        if signature.return_type.is_none() {
            self.report(
                DiagnosticCode::ImplicitAnyConstructSignature,
                messages::construct_signature_lacks_return_annotation(),
                signature.span,
            );
        }
        walk::walk_ts_construct_signature_declaration(self, signature);
    }

    fn visit_ts_function_type(&mut self, function: &TSFunctionType<'a>) {
        self.params(&function.params);
        walk::walk_ts_function_type(self, function);
    }

    fn visit_ts_constructor_type(&mut self, constructor: &TSConstructorType<'a>) {
        self.params(&constructor.params);
        walk::walk_ts_constructor_type(self, constructor);
    }
}

// The names `this.x = ...`, `this["x"] = ...` and `this[0] = ...` assign anywhere in a body.
#[derive(Default)]
struct ThisAssignments {
    names: std::collections::HashSet<String>,
}

impl<'a> Visit<'a> for ThisAssignments {
    fn visit_assignment_expression(&mut self, assignment: &oxc_ast::ast::AssignmentExpression<'a>) {
        use oxc_ast::ast::{AssignmentTarget, Expression};
        match &assignment.left {
            AssignmentTarget::StaticMemberExpression(member)
                if matches!(member.object, Expression::ThisExpression(_)) =>
            {
                self.names.insert(member.property.name.to_string());
            }
            AssignmentTarget::ComputedMemberExpression(member)
                if matches!(member.object, Expression::ThisExpression(_)) =>
            {
                match &member.expression {
                    Expression::StringLiteral(lit) => {
                        self.names.insert(lit.value.to_string());
                    }
                    Expression::NumericLiteral(lit) => {
                        self.names.insert(lit.value.to_string());
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        walk::walk_assignment_expression(self, assignment);
    }
}
