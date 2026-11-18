//! The two rewrites that need what the checker knows (`tsox-model`), done on the tree before
//! the transformer strips types.
//!
//! Import elision: an import declaration whose specifier the model says is used only as a type
//! (or not at all) is dropped. A bare `import "x"` is `Value` in the model, so it stays.
//!
//! `const enum` inlining: a reference `E.A` or `E["A"]` to a `const enum` declared in this file
//! becomes the member's value from `const_values`. A `const enum` that is not exported and has
//! no reference left is removed, as tsc does. An exported one stays, since another file's
//! output may still read it (cross-file inlining is not done here).

use oxc_allocator::Allocator;
use oxc_ast::ast::*;
use oxc_ast::builder::AstBuilder;
use oxc_ast_visit::VisitMut;
use oxc_span::SPAN;
use tsox_base::keys::ModuleKey;
use tsox_model::{ConstValue, ImportUse, SemanticModel, enum_member_key};

pub(crate) fn apply<'a>(
    allocator: &'a Allocator,
    program: &mut Program<'a>,
    model: &SemanticModel,
    module: Option<ModuleKey>,
) {
    program.body.retain(|statement| {
        let Statement::ImportDeclaration(import) = statement else {
            return true;
        };
        !model.module_info.imports.iter().any(|(specifier, usage)| {
            **specifier == *import.source.value.as_str() && *usage == ImportUse::TypeOnly
        })
    });
    let Some(module) = module else { return };
    let mut local: Vec<(String, bool)> = Vec::new();
    for statement in &program.body {
        match statement {
            Statement::TSEnumDeclaration(decl) if decl.r#const => {
                local.push((decl.id.name.to_string(), false));
            }
            Statement::ExportDeclaration(export) => {
                if let Declaration::TSEnumDeclaration(decl) = &export.declaration {
                    if decl.r#const {
                        local.push((decl.id.name.to_string(), true));
                    }
                }
            }
            _ => {}
        }
    }
    if local.is_empty() {
        return;
    }
    let mut inliner = Inliner {
        ast: AstBuilder::new(allocator),
        allocator,
        model,
        module,
        names: local.iter().map(|(name, _)| name.clone()).collect(),
        skipped: Vec::new(),
    };
    inliner.visit_program(program);
    program.body.retain(|statement| match statement {
        Statement::TSEnumDeclaration(decl) if decl.r#const => {
            inliner.skipped.contains(&decl.id.name.to_string())
        }
        _ => true,
    });
}

struct Inliner<'a, 'm> {
    ast: AstBuilder<'a>,
    allocator: &'a Allocator,
    model: &'m SemanticModel,
    module: ModuleKey,
    names: Vec<String>,
    /// Enums with a reference that could not be replaced, which keep their declaration.
    skipped: Vec<String>,
}

impl<'a> Inliner<'a, '_> {
    fn value_of(&self, enum_name: &str, member: &str) -> Option<&ConstValue> {
        self.model
            .const_values
            .get(&enum_member_key(self.module, enum_name, member))
    }

    fn literal(&self, value: &ConstValue) -> Option<Expression<'a>> {
        match value {
            ConstValue::Number(bits) => {
                let number = f64::from_bits(*bits);
                if !number.is_finite() {
                    return None;
                }
                let positive = Expression::new_numeric_literal(
                    SPAN,
                    number.abs(),
                    None,
                    NumberBase::Decimal,
                    &self.ast,
                );
                Some(if number.is_sign_negative() && number != 0.0 {
                    Expression::new_unary_expression(
                        SPAN,
                        UnaryOperator::UnaryNegation,
                        positive,
                        &self.ast,
                    )
                } else {
                    positive
                })
            }
            ConstValue::String(text) => Some(Expression::new_string_literal(
                SPAN,
                &*self.allocator.alloc_str(text),
                None,
                &self.ast,
            )),
            ConstValue::Boolean(_) => None,
        }
    }
}

impl<'a> VisitMut<'a> for Inliner<'a, '_> {
    fn visit_expression(&mut self, expression: &mut Expression<'a>) {
        let named = match &*expression {
            Expression::StaticMemberExpression(member) => match &member.object {
                Expression::Identifier(object) => {
                    Some((object.name.to_string(), member.property.name.to_string()))
                }
                _ => None,
            },
            Expression::ComputedMemberExpression(member) => {
                match (&member.object, &member.expression) {
                    (Expression::Identifier(object), Expression::StringLiteral(key)) => {
                        Some((object.name.to_string(), key.value.to_string()))
                    }
                    _ => None,
                }
            }
            _ => None,
        };
        if let Some((enum_name, member)) = named {
            if self.names.contains(&enum_name) {
                match self
                    .value_of(&enum_name, &member)
                    .and_then(|v| self.literal(v))
                {
                    Some(replacement) => {
                        *expression = replacement;
                        return;
                    }
                    None => self.skipped.push(enum_name),
                }
            }
        }
        oxc_ast_visit::walk_mut::walk_expression(self, expression);
    }
}
