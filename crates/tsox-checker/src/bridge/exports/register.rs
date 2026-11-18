//! Registration of pending exports and their nested signature keys.
use super::*;

pub(super) fn register<'ast>(
    keys: &mut UnitKeys,
    ctx: &mut CheckContext<'ast, '_>,
    mut p: Pending,
) -> Option<Pending> {
    if matches!(
        p.kind,
        DeclKind::Interface | DeclKind::Alias | DeclKind::Class
    ) {
        let declared = ctx.namespace.declared_type_param_decl(&p.name);
        p = with_type_params(ctx, p, declared)?;
        // The slot the rest of the unit knows this declaration by.
        if let Resolution::Resolved(type_id) = ctx.namespace.resolve(&p.name, &mut ctx.arena) {
            if let Some((slot, _)) = ctx.arena.decl_parts(type_id) {
                keys.slots.insert(slot, (p.key, p.name.clone()));
            } else if p.kind == DeclKind::Alias && !p.type_params.is_empty() {
                // A generic alias has no slot of its own, only its body; applications of
                // it are traced back to this key through the body.
                let slot = ctx.arena.alloc_name(&p.name);
                keys.slots.insert(slot, (p.key, p.name.clone()));
                ctx.arena.set_body_origin(type_id, slot);
            }
        }
    }
    for (i, tp) in p.type_params.iter().enumerate() {
        keys.tparams.insert(
            tp.id,
            TypeParamKey {
                owner: p.key,
                index: i as u32,
            },
        );
    }
    // Generic function types can appear inside parameters, returns, object members,
    // aliases, and constraints. They have no top-level export declaration of their own,
    // so give their type parameters stable child keys before hashing or serializing the
    // body. Top-level signature parameters above keep the declaration key itself. The key
    // is the declaration's own key plus the path down to the signature, never a position
    // in the file.
    let parent = p.key;
    let mut path = Vec::new();
    register_nested_signature_keys(keys, &ctx.arena, &ctx.importer, parent, p.body, &mut path);
    for (index, parameter) in p.type_params.iter().enumerate() {
        for (tag, root) in [
            (TAG_DECL_BOUND, parameter.bound),
            (TAG_DECL_DEFAULT, parameter.default),
        ] {
            if let Some(root) = root {
                path.clear();
                path.push(tag);
                path.extend((index as u32).to_le_bytes());
                register_nested_signature_keys(
                    keys,
                    &ctx.arena,
                    &ctx.importer,
                    parent,
                    root,
                    &mut path,
                );
            }
        }
    }
    Some(p)
}

// One byte per step from a declaration's type down to a nested type, with a number where
// the step has an index, so two signatures in one declaration never share a path.
const TAG_PROPERTY: u8 = 1;
const TAG_PARAMETER: u8 = 2;
const TAG_RETURN: u8 = 3;
const TAG_MEMBER: u8 = 4;
const TAG_ELEMENT: u8 = 5;
const TAG_BOUND: u8 = 6;
const TAG_DEFAULT: u8 = 7;
const TAG_ARGUMENT: u8 = 8;
const TAG_INNER: u8 = 9;
const TAG_GENERIC_BOUND: u8 = 10;
const TAG_DECL_BOUND: u8 = 11;
const TAG_DECL_DEFAULT: u8 = 12;

pub(super) fn register_nested_signature_keys(
    keys: &mut UnitKeys,
    arena: &crate::arena::TypeArena,
    imported: &Importer,
    parent: DeclKey,
    root: TypeId,
    path: &mut Vec<u8>,
) {
    struct Walk<'a> {
        keys: &'a mut UnitKeys,
        arena: &'a crate::arena::TypeArena,
        imported: &'a Importer,
        parent: DeclKey,
        // Membership only; it is never iterated, so its order cannot reach the output.
        seen: std::collections::HashSet<TypeId>,
    }

    // Runs `body` one step further down, and puts the path back afterwards.
    fn step(path: &mut Vec<u8>, tag: u8, index: Option<u32>, body: impl FnOnce(&mut Vec<u8>)) {
        let length = path.len();
        path.push(tag);
        if let Some(index) = index {
            path.extend(index.to_le_bytes());
        }
        body(path);
        path.truncate(length);
    }

    impl Walk<'_> {
        fn visit(&mut self, id: TypeId, path: &mut Vec<u8>) {
            if !self.seen.insert(id) {
                return;
            }
            match self.arena.raw(id) {
                Type::Function(function) => {
                    if !function.type_params.is_empty() {
                        let owner = DeclKey::nested_type_signature(self.parent, path);
                        for (index, parameter) in function.type_params.iter().enumerate() {
                            if self.keys.tparams.contains_key(&parameter.id)
                                || self.imported.type_param_key(parameter.id).is_some()
                            {
                                continue;
                            }
                            self.keys.tparams.insert(
                                parameter.id,
                                TypeParamKey {
                                    owner,
                                    index: index as u32,
                                },
                            );
                        }
                    }
                    for (index, parameter) in function.type_params.iter().enumerate() {
                        let index = index as u32;
                        if let Some(constraint) = parameter.constraint {
                            step(path, TAG_BOUND, Some(index), |p| self.visit(constraint, p));
                        }
                        if let Some(default) = parameter.default {
                            step(path, TAG_DEFAULT, Some(index), |p| self.visit(default, p));
                        }
                    }
                    for (index, parameter) in function.params.iter().enumerate() {
                        step(path, TAG_PARAMETER, Some(index as u32), |p| {
                            self.visit(parameter.type_id, p)
                        });
                    }
                    step(path, TAG_RETURN, None, |p| {
                        self.visit(function.return_type, p)
                    });
                }
                Type::Object(object) => {
                    for property in object.properties.iter() {
                        // The name is the step: properties are sorted by name, and a name
                        // does not move when the declaration does.
                        let length = path.len();
                        path.push(TAG_PROPERTY);
                        path.extend((property.name.len() as u32).to_le_bytes());
                        path.extend(property.name.as_bytes());
                        self.visit(property.type_id, path);
                        path.truncate(length);
                    }
                }
                Type::Array(element) => step(path, TAG_ELEMENT, None, |p| self.visit(*element, p)),
                Type::Tuple(elements) => {
                    for (index, element) in elements.iter().enumerate() {
                        step(path, TAG_ELEMENT, Some(index as u32), |p| {
                            self.visit(element.type_id, p)
                        });
                    }
                }
                Type::Union(members) | Type::Intersection(members) => {
                    for (index, &member) in members.iter().enumerate() {
                        step(path, TAG_MEMBER, Some(index as u32), |p| {
                            self.visit(member, p)
                        });
                    }
                }
                Type::GenericParameter(_, _, bound) => {
                    if let Some(bound) = bound {
                        step(path, TAG_GENERIC_BOUND, None, |p| self.visit(*bound, p));
                    }
                }
                Type::Named(_, inner) => step(path, TAG_INNER, None, |p| self.visit(*inner, p)),
                // `keyof T` and `T[K]`: each operand is a step of its own under the member tag.
                Type::Keyof(operand) | Type::StringMapping(_, operand) => {
                    step(path, TAG_MEMBER, Some(0), |p| self.visit(*operand, p));
                }
                Type::Variadic(elements) => {
                    for (position, element) in elements.iter().enumerate() {
                        step(path, TAG_MEMBER, Some(position as u32), |p| {
                            self.visit(element.type_id, p)
                        });
                    }
                }
                Type::Template(template) => {
                    for (position, part) in template.types.iter().enumerate() {
                        step(path, TAG_MEMBER, Some(position as u32), |p| {
                            self.visit(*part, p)
                        });
                    }
                }
                Type::Mapped(mapped) => {
                    for (position, part) in [
                        Some(mapped.constraint),
                        mapped.name_type,
                        Some(mapped.value),
                    ]
                    .into_iter()
                    .flatten()
                    .enumerate()
                    {
                        step(path, TAG_MEMBER, Some(position as u32), |p| {
                            self.visit(part, p)
                        });
                    }
                }
                Type::IndexedAccess(object, index) => {
                    for (position, part) in [*object, *index].into_iter().enumerate() {
                        step(path, TAG_MEMBER, Some(position as u32), |p| {
                            self.visit(part, p)
                        });
                    }
                }
                // A conditional's four parts, each a step of its own under the member tag.
                Type::Conditional(conditional) => {
                    for (index, part) in [
                        conditional.check_type,
                        conditional.extends_type,
                        conditional.true_type,
                        conditional.false_type,
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        step(path, TAG_MEMBER, Some(index as u32), |p| {
                            self.visit(part, p)
                        });
                    }
                }
                // An App's body belongs to its declaration, which is registered separately;
                // the application only introduces its own arguments at this use site.
                Type::App(_, arguments, _body) => {
                    for (index, &argument) in arguments.iter().enumerate() {
                        step(path, TAG_ARGUMENT, Some(index as u32), |p| {
                            self.visit(argument, p)
                        });
                    }
                }
                // A Ref intentionally stops at a declaration boundary. Its body receives
                // keys while its own declaration is registered.
                Type::Number
                | Type::String
                | Type::Boolean
                | Type::Null
                | Type::Undefined
                | Type::Any
                | Type::Unknown
                | Type::NonPrimitive
                | Type::Error
                | Type::Never
                | Type::Void
                | Type::StringLiteral(_)
                | Type::NumberLiteral(_)
                | Type::BooleanLiteral(_)
                | Type::Ref(_) => {}
            }
        }
    }

    let mut walk = Walk {
        keys,
        arena,
        imported,
        parent,
        seen: std::collections::HashSet::new(),
    };
    walk.visit(root, path);
}

// The declaration's own `<T extends B = D, ...>` list as export inputs. The bound is read
// off the GenericParameter node the declare pass already allocated for the parameter, so
// it is the very type the body was resolved against. The default is resolved again with
// the declaration's parameters in scope (the namespace memoizes the parameter nodes, so
// a default that names an earlier parameter gets that same node). None when a parameter
// has no node, which means the declaration was never resolved with it in scope.
pub(super) fn with_type_params<'ast>(
    ctx: &mut CheckContext<'ast, '_>,
    mut p: Pending,
    declared: Option<&'ast TSTypeParameterDeclaration<'ast>>,
) -> Option<Pending> {
    let Some(declared) = declared else {
        return Some(p);
    };
    let file_id = ctx.namespace.file_id();
    for (i, param) in declared.params.iter().enumerate() {
        let id = TypeParameterId::with_file(file_id, param.span().start, i as u32);
        let node = ctx.namespace.type_param_node(id)?;
        let Type::GenericParameter(_, _, bound) = ctx.arena.raw(node) else {
            return None;
        };
        let bound = *bound;
        let default = if param.default.is_some() {
            // A default that cannot be resolved cannot be exported.
            Some(
                ctx.namespace
                    .resolve_type_param_default(&mut ctx.arena, declared, i)?,
            )
        } else {
            None
        };
        p.type_params.push(TypeParamInput {
            id,
            name: param.name.name.to_string(),
            bound,
            default,
        });
    }
    Some(p)
}
