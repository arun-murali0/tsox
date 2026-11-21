use std::collections::HashMap;

use oxc_ast::ast::{
    Class, ClassElement, MethodDefinitionKind, Program, PropertyKey, TSInterfaceDeclaration,
    TSSignature, TSTypeLiteral, TSTypeParameterDeclaration,
};
use oxc_ast_visit::{Visit, walk};
use oxc_span::{GetSpan, Span};

use crate::diagnostic_messages::messages;

use super::context::CheckContext;

// Problem: two members of one class, interface or type literal with the same name
// (`a(): number; a: number;`, `"a b": number; "a b": number;`, `1: number; 1.0: number;`) and a
// type parameter list that names one parameter twice (`class C<T, T>`) went unreported
// (TS2300).
// Picked: the rules read off tsc's own output. In a class, a property after a member of its name
// is reported at the property; a method after a property is reported at both, and two methods
// are overloads. In an interface or type literal every member of a clashing name is reported,
// two methods again being overloads. A repeated type parameter is reported at each occurrence.
// Names compare as the property they make (`1`, `1.0` and `"1"` are one name). A class names the
// member as it was written, the others by the property name.
// Cost: accessors are not compared, and neither are members of separate declarations of one
// merged interface.
pub fn report_duplicate_members<'ast>(
    program: &'ast Program<'ast>,
    ctx: &mut CheckContext<'ast, '_>,
) {
    let mut walker = Walker { ctx };
    walker.visit_program(program);
}

struct Walker<'c, 'ast, 'src> {
    ctx: &'c mut CheckContext<'ast, 'src>,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Property,
    Method,
}

struct Member {
    name: String,
    written: String,
    kind: Kind,
    is_static: bool,
    span: Span,
}

fn canonical_name(key: &PropertyKey<'_>) -> Option<String> {
    match key {
        PropertyKey::StaticIdentifier(id) => Some(id.name.to_string()),
        PropertyKey::StringLiteral(s) => Some(s.value.to_string()),
        PropertyKey::NumericLiteral(n) => Some(number_name(n.value)),
        _ => None,
    }
}

// The property name a number literal makes: `1.0` and `1.` are `1`.
fn number_name(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e21 {
        format!("{}", value as i128)
    } else {
        format!("{value}")
    }
}

impl Walker<'_, '_, '_> {
    fn text(&self, span: Span) -> String {
        self.ctx
            .source_text
            .get(span.start as usize..span.end as usize)
            .unwrap_or_default()
            .to_string()
    }

    fn report(&mut self, text: &str, span: Span) {
        self.ctx
            .error(messages::duplicate_type_declaration(text), span);
    }

    fn check_class(&mut self, class: &Class<'_>) {
        let mut seen: Vec<Member> = Vec::new();
        for element in &class.body.body {
            let (key, kind, is_static) = match element {
                ClassElement::PropertyDefinition(p) => (&p.key, Kind::Property, p.r#static),
                ClassElement::MethodDefinition(m) => match m.kind {
                    MethodDefinitionKind::Method => (&m.key, Kind::Method, m.r#static),
                    _ => continue,
                },
                _ => continue,
            };
            let Some(name) = canonical_name(key) else {
                continue;
            };
            let span = key.span();
            let member = Member {
                written: self.text(span),
                name,
                kind,
                is_static,
                span,
            };
            let earlier: Vec<usize> = seen
                .iter()
                .enumerate()
                .filter(|(_, m)| m.name == member.name && m.is_static == member.is_static)
                .map(|(index, _)| index)
                .collect();
            if !earlier.is_empty() {
                let clash = member.kind == Kind::Property
                    || earlier.iter().any(|&i| seen[i].kind == Kind::Property);
                if clash {
                    // A property is reported alone; a method also reports the properties it
                    // follows.
                    if member.kind == Kind::Method {
                        for &i in &earlier {
                            if seen[i].kind == Kind::Property {
                                let (written, span) = (seen[i].written.clone(), seen[i].span);
                                self.report(&written, span);
                            }
                        }
                    }
                    self.report(&member.written, member.span);
                }
            }
            seen.push(member);
        }
    }

    // The members of an interface or a type literal: every one of a clashing name is reported.
    fn check_signatures(&mut self, members: &[TSSignature<'_>]) {
        let mut by_name: HashMap<String, Vec<(Kind, Span)>> = HashMap::new();
        let mut order: Vec<String> = Vec::new();
        for member in members {
            let (key, kind) = match member {
                TSSignature::TSPropertySignature(p) => (&p.key, Kind::Property),
                TSSignature::TSMethodSignature(m)
                    if matches!(m.kind, oxc_ast::ast::TSMethodSignatureKind::Method) =>
                {
                    (&m.key, Kind::Method)
                }
                _ => continue,
            };
            let Some(name) = canonical_name(key) else {
                continue;
            };
            if !by_name.contains_key(&name) {
                order.push(name.clone());
            }
            by_name.entry(name).or_default().push((kind, key.span()));
        }
        for name in order {
            let group = &by_name[&name];
            let clash = group.len() > 1 && group.iter().any(|(kind, _)| *kind == Kind::Property);
            if clash {
                for &(_, span) in group {
                    self.report(&name, span);
                }
            }
        }
    }

    fn check_type_parameters(&mut self, list: &TSTypeParameterDeclaration<'_>) {
        for (index, parameter) in list.params.iter().enumerate() {
            let name = parameter.name.name.as_str();
            let repeated = list
                .params
                .iter()
                .enumerate()
                .any(|(other, p)| other != index && p.name.name == name);
            if repeated {
                self.report(name, parameter.name.span);
            }
        }
    }
}

impl<'ast> Visit<'ast> for Walker<'_, 'ast, '_> {
    fn visit_class(&mut self, class: &Class<'ast>) {
        self.check_class(class);
        walk::walk_class(self, class);
    }

    fn visit_ts_interface_declaration(&mut self, decl: &TSInterfaceDeclaration<'ast>) {
        self.check_signatures(&decl.body.body);
        walk::walk_ts_interface_declaration(self, decl);
    }

    fn visit_ts_type_literal(&mut self, literal: &TSTypeLiteral<'ast>) {
        self.check_signatures(&literal.members);
        walk::walk_ts_type_literal(self, literal);
    }

    fn visit_ts_type_parameter_declaration(&mut self, list: &TSTypeParameterDeclaration<'ast>) {
        self.check_type_parameters(list);
        walk::walk_ts_type_parameter_declaration(self, list);
    }
}
