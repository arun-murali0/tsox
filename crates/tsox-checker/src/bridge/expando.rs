// Expando properties: `f.x = 1` after `function f() {}` (or a const holding a function or
// arrow) adds `x` to the type of `f`. Found syntactically once per file; the property
// is typed `any`, so reads and writes of it are accepted and nothing is guessed.
use std::collections::HashMap;

use oxc_ast::ast::{
    AssignmentExpression, AssignmentOperator, AssignmentTarget, Expression, Program,
};
use oxc_ast_visit::{Visit, walk};
use oxc_semantic::{Scoping, SymbolId};

use super::context::CheckContext;
use super::narrow::resolve_symbol_id;
use crate::arena::TypeId;
use crate::types::{ObjectType, PropertyEntry, Type};

pub type ExpandoMap = HashMap<SymbolId, Vec<String>>;

struct Collector<'s> {
    scoping: &'s Scoping,
    found: ExpandoMap,
}

impl<'a> Visit<'a> for Collector<'_> {
    fn visit_assignment_expression(&mut self, expr: &AssignmentExpression<'a>) {
        if expr.operator == AssignmentOperator::Assign {
            let target = match &expr.left {
                AssignmentTarget::StaticMemberExpression(m) => {
                    Some((&m.object, m.property.name.to_string()))
                }
                AssignmentTarget::ComputedMemberExpression(m) => match &m.expression {
                    Expression::StringLiteral(s) => Some((&m.object, s.value.to_string())),
                    _ => None,
                },
                _ => None,
            };
            if let Some((Expression::Identifier(id), name)) = target
                && let Some(symbol) = resolve_symbol_id(id, self.scoping)
            {
                let names = self.found.entry(symbol).or_default();
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }
        walk::walk_assignment_expression(self, expr);
    }
}

pub fn collect(program: &Program, scoping: &Scoping) -> ExpandoMap {
    let mut collector = Collector {
        scoping,
        found: ExpandoMap::new(),
    };
    collector.visit_program(program);
    collector.found
}

pub fn apply(symbol: SymbolId, base: TypeId, ctx: &mut CheckContext<'_, '_>) -> TypeId {
    let Some(names) = ctx.expandos.get(&symbol) else {
        return base;
    };
    let has_fn = match ctx.arena.get(base) {
        Type::Function(_) => true,
        Type::Intersection(ms) => ms
            .iter()
            .any(|&m| matches!(ctx.arena.get(m), Type::Function(_))),
        _ => false,
    };
    if !has_fn {
        return base;
    }
    let any = ctx.arena.any();
    let props = names
        .clone()
        .into_iter()
        .map(|n| PropertyEntry {
            name: n.as_str().into(),
            type_id: any,
            optional: false,
            is_method: false,
            modifiers: Default::default(),
        })
        .collect::<Vec<_>>();
    let obj = ctx.arena.alloc(Type::Object(ObjectType::new(props)));
    ctx.arena
        .alloc_intersection(vec![base, obj])
        .unwrap_or(base)
}
