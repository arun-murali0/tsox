use oxc_ast::ast::{
    BindingPattern, FormalParameters, PropertyKey, TSMethodSignature, TSMethodSignatureKind,
    TSSignature, TSType, TSTypeAnnotation, TSTypeName,
};
use oxc_span::GetSpan;

use crate::arena::{TypeArena, TypeId};
use crate::namespace::{Resolution, TypeNamespace};
use crate::types::{
    FunctionType, FunctionTypeParam, ObjectType, Param, PropertyEntry, Type, TypeParameterId,
};

pub fn resolve_type_annotation(
    annotation: &TSTypeAnnotation,
    namespace: &mut TypeNamespace,
    arena: &mut TypeArena,
) -> Option<TypeId> {
    resolve_ts_type(&annotation.type_annotation, namespace, arena)
}

/// Resolve the explicit type-parameter list owned by a function or method signature.
/// The caller must have pushed this declaration's type-parameter scope already, so
/// constraints/defaults resolve to the same stable GenericParameter nodes as the
/// parameter and return annotations.
pub(crate) fn resolve_function_type_params(
    declared: Option<&oxc_ast::ast::TSTypeParameterDeclaration<'_>>,
    namespace: &mut TypeNamespace,
    arena: &mut TypeArena,
) -> Vec<FunctionTypeParam> {
    let Some(declared) = declared else {
        return Vec::new();
    };
    let file_id = namespace.file_id();
    declared
        .params
        .iter()
        .enumerate()
        .map(|(index, param)| {
            let id = TypeParameterId::with_file(file_id, param.span().start, index as u32);
            let constraint = namespace.type_param_constraint(arena, id);
            let default = if param.default.is_some() {
                namespace.resolve_type_param_default(arena, declared, index)
            } else {
                None
            };
            FunctionTypeParam {
                id,
                name: param.name.name.to_string(),
                constraint,
                default,
            }
        })
        .collect()
}

pub fn resolve_ts_type(
    ty: &TSType,
    namespace: &mut TypeNamespace,
    arena: &mut TypeArena,
) -> Option<TypeId> {
    // A type written as `number[][][]...` nests as deep as the file lets it, and each level
    // is a call here. Past the stack budget the type is unknown and the file says so once.
    if crate::stack_guard::exhausted() {
        namespace.note_depth_cut();
        return Some(arena.error());
    }
    match ty {
        TSType::TSNumberKeyword(_) => Some(arena.number()),
        TSType::TSStringKeyword(_) => Some(arena.string()),
        TSType::TSBooleanKeyword(_) => Some(arena.boolean()),
        TSType::TSNullKeyword(_) => Some(arena.null()),
        TSType::TSUndefinedKeyword(_) => Some(arena.undefined()),
        TSType::TSAnyKeyword(_) => Some(arena.any()),
        TSType::TSUnknownKeyword(_) => Some(arena.unknown()),
        TSType::TSNeverKeyword(_) => Some(arena.never()),
        TSType::TSVoidKeyword(_) => Some(arena.void()),

        TSType::TSArrayType(array) => {
            let element = resolve_ts_type(&array.element_type, namespace, arena)?;
            Some(arena.alloc(Type::Array(element)))
        }

        TSType::TSUnionType(union) => {
            let mut members = Vec::with_capacity(union.types.len());
            for member in &union.types {
                members.push(resolve_ts_type(member, namespace, arena)?);
            }
            Some(arena.alloc_union(members))
        }

        // `A & B`. alloc_intersection does the work (LLD 1.13): flattening, the
        // primitive rules, distribution over unions. One that would distribute into too
        // many members is TS2590 in tsc. It reads as the error type, which suppresses what
        // would follow from it, and its span is noted on the namespace for the checker to
        // report, since resolving an annotation has no diagnostics of its own to write to.
        TSType::TSIntersectionType(intersection) => {
            let mut members = Vec::with_capacity(intersection.types.len());
            for member in &intersection.types {
                members.push(resolve_ts_type(member, namespace, arena)?);
            }
            match arena.alloc_intersection(members) {
                Ok(id) => Some(id),
                Err(_) => {
                    namespace.note_too_complex(intersection.span);
                    Some(arena.error())
                }
            }
        }

        TSType::TSTypeLiteral(literal) => {
            // resolve_object_members takes references to signatures rather than
            // owned ones so the interface-merging call site in namespace.rs can
            // pass members gathered from more than one declaration; a plain type
            // literal only ever has the one Vec of its own, so this collects a
            // Vec of references to it just to match that shared shape.
            let members: Vec<&TSSignature> = literal.members.iter().collect();
            resolve_object_members(&members, namespace, arena)
        }

        TSType::TSFunctionType(func_type) => {
            let scope =
                namespace.push_decl_type_params(arena, func_type.type_parameters.as_deref());
            namespace.note_implicit_any_params(&func_type.params);
            let params = resolve_params_with_any_fallback(&func_type.params, namespace, arena);
            let return_type = resolve_type_annotation(&func_type.return_type, namespace, arena);
            let type_params = resolve_function_type_params(
                func_type.type_parameters.as_deref(),
                namespace,
                arena,
            );
            namespace.pop_type_params(scope);
            Some(arena.alloc(Type::Function(crate::types::FunctionType {
                params,
                return_type: return_type?,
                type_params,
                is_untyped: false,
            })))
        }

        // Problem: `(string | null)[]` was unresolvable, because oxc keeps the parentheses
        // as their own node and nothing here looked through it. Any function with such a
        // parameter was left undeclared and its body went unchecked.
        // Picked: a parenthesized type is the type inside it.
        TSType::TSParenthesizedType(inner) => {
            resolve_ts_type(&inner.type_annotation, namespace, arena)
        }

        TSType::TSLiteralType(literal) => resolve_literal_type(&literal.literal, arena),

        TSType::TSTypeReference(reference) => {
            // `N.I` is looked up under its dotted name, which is how a declaration inside
            // `namespace N` is registered. Anything deeper than a plain dotted path of
            // identifiers (`this.x`) is not a type name this resolves.
            let dotted = dotted_type_name(&reference.type_name)?;
            let id = DottedName {
                name: dotted.as_str(),
            };

            // A handful of TypeScript's own built-in generics aren't declared
            // anywhere in user source, so the namespace would never find them.
            // Only consulted when the namespace doesn't already claim the name,
            // so an unusual user-defined `interface Array { ... }` is never
            // shadowed by this.
            if !namespace.contains(id.name) {
                if let Some(builtin) = resolve_builtin_generic(id.name, reference, namespace, arena)
                {
                    return Some(builtin);
                }
            }

            let base = match namespace.resolve(id.name, arena) {
                Resolution::Resolved(type_id) => type_id,
                Resolution::Circular | Resolution::NotFound => return None,
            };

            // A reference whose type argument count disagrees with its
            // declaration is recorded, not rejected here: resolution carries on
            // leniently below, and the mismatch is reported afterward (see
            // bridge::check_program). Parameters with a default may be omitted,
            // so the valid range is required..=total, as in tsc.
            let given = reference
                .type_arguments
                .as_ref()
                .map_or(0, |arguments| arguments.params.len());
            let arity = namespace.declared_type_param_arity(id.name);
            if let Some((required, total)) = arity {
                if given < required || given > total {
                    namespace.note_type_argument_issue(id.name, required, total, reference.span);
                }
            }

            // Not generic, or a declaration this checker does not model (a
            // class): nothing to substitute.
            // A generic imported from another file has no AST: its parameters come
            // from the import record instead (see TypeNamespace::insert_imported), and
            // everything after the bindings is shared with a local declaration.
            let imported = namespace.imported_generic(id.name);
            let decl = namespace.declared_type_param_decl(id.name);
            if imported.is_none() && decl.is_none() {
                return Some(base);
            }

            // A bare `Box` for `interface Box<T>` was reported above. As in tsc
            // the reference is then the error type, which is compatible with
            // everything, so the missing arguments cannot cascade into a second,
            // unrelated diagnostic and no bare placeholder leaks out.
            if given == 0 && arity.is_some_and(|(required, _)| required > 0) {
                return Some(arena.error());
            }

            // base is the declaration's own cached generic shape, still holding
            // bare GenericParameter placeholders. One binding per *declared*
            // parameter, in declaration order, keyed by the same TypeParameterId
            // push_decl_type_params uses, so positions cannot drift: a parameter
            // the body never mentions still owns its slot, and an argument that
            // cannot be resolved becomes the error type in place instead of
            // being dropped and shifting every later argument left. Each
            // explicit argument is resolved in the *caller's* namespace, exactly
            // like a generic call's explicit type arguments in calls.rs.
            // Arguments beyond the declared count are ignored; an omitted one
            // takes its declared default, or the error type if it has none.
            let mut bindings: Vec<(TypeParameterId, TypeId)> = Vec::new();
            if let Some(imported) = &imported {
                for (index, param) in imported.params.iter().enumerate() {
                    let bound = match reference
                        .type_arguments
                        .as_ref()
                        .and_then(|arguments| arguments.params.get(index))
                    {
                        Some(argument) => {
                            let resolved = resolve_ts_type(argument, namespace, arena)
                                .unwrap_or_else(|| arena.error());
                            check_type_argument_constraint(
                                namespace,
                                arena,
                                (param.id, param.name.as_str()),
                                resolved,
                                &bindings,
                                argument.span(),
                            );
                            resolved
                        }
                        // The default was resolved in the declaring file; it may name
                        // an earlier parameter, which the arguments bound so far fill.
                        None => param
                            .default
                            .map(|default| {
                                crate::semantic::substitute_type_params(arena, default, &bindings)
                            })
                            .unwrap_or_else(|| arena.error()),
                    };
                    bindings.push((param.id, bound));
                }
            }
            if let Some(decl) = decl {
                for (index, param) in decl.params.iter().enumerate() {
                    let parameter_id = TypeParameterId::with_file(
                        namespace.file_id(),
                        param.span().start,
                        index as u32,
                    );
                    let bound = match reference
                        .type_arguments
                        .as_ref()
                        .and_then(|arguments| arguments.params.get(index))
                    {
                        Some(argument) => {
                            let resolved = resolve_ts_type(argument, namespace, arena)
                                .unwrap_or_else(|| arena.error());
                            check_type_argument_constraint(
                                namespace,
                                arena,
                                (parameter_id, param.name.name.as_str()),
                                resolved,
                                &bindings,
                                argument.span(),
                            );
                            resolved
                        }
                        None => namespace
                            .resolve_type_param_default(arena, decl, index)
                            .map(|default| {
                                crate::semantic::substitute_type_params(arena, default, &bindings)
                            })
                            .unwrap_or_else(|| arena.error()),
                    };
                    bindings.push((parameter_id, bound));
                }
            }

            // Everything above is per reference and stays per reference: the arity
            // and constraint checks report at this site's own span. Only the work
            // below, which depends on nothing but the declaration and its resolved
            // arguments, is reused.
            //
            // A hit is the stored application itself. It carries no name that a caller
            // could change (`type Alias = Box<number>` wraps it, it does not rename it),
            // and it prints from its arguments when it is displayed, so there is
            // nothing a later reference could find out of date.
            if let Some(application) = namespace.cached_instantiation(base, &bindings) {
                return Some(application);
            }

            // Only the declaration's own parameters are bound here, so a parameter
            // a member declares for itself (`map<U>(...)`) is kept for inference
            // at the call site instead of becoming unknown.
            let result = crate::semantic::substitute_bound_type_params(arena, base, &bindings);

            // Prints as `Box<number>`, not Box's full member list, in a message.
            // Skipped when substitution left `result` identical to `base`: that
            // happens when none of Box's declared parameters actually appear in
            // its body, in which case every instantiation of Box shares this one
            // TypeId, and there is no application of it worth telling apart.
            if result != base {
                // The application is its own node: the declaration's name, the
                // arguments, and `result` as what it is. Substitution allocates through
                // alloc(), so `result` may be the id every identical anonymous shape
                // shares (a Box<number> and a Pair<number> with the same members, or a
                // plain `{ value: number }`); that id is not renamed, it is wrapped.
                // Each argument prints in its own current spelling when the
                // application is displayed, so `Box<Dog>` says `Box<Dog>` whether Dog
                // had its name yet when this was built or not.
                let slot = arena.alloc_name(id.name);
                let arguments = bindings.iter().map(|&(_, bound)| bound).collect();
                let application = arena.alloc_app(slot, arguments, result);

                // Stored only here, inside `result != base`: an unchanged result
                // means `base` was still an unresolved Ref (a reference from inside
                // its own declaration), and remembering that would return the empty
                // shape for every later reference, after it is resolved.
                namespace.cache_instantiation(base, bindings, application);
                return Some(application);
            }

            Some(result)
        }

        _ => None,
    }
}

// Checks one explicit type argument against the `extends` bound of the parameter
// it was given to, the way tsc does (TS2344), and records a violation for
// bridge::check_program to report. The bound may mention earlier parameters
// (`<T, U extends T>`), so it is substituted with the arguments bound so far.
//
// Skipped when the bound itself still mentions a type parameter that has no
// binding yet (a bound written in terms of a parameter that was not supplied):
// there is nothing concrete to compare against. An argument that is a type
// parameter of the enclosing declaration is compared normally, through its own
// bound (see the GenericParameter arm in subtyping.rs), so
// `function f<T extends string>(b: Box<T>)` passes for `Box<U extends string>`
// while an unconstrained T does not, as in tsc. An argument that failed to
// resolve is the error type, which is compatible with everything.
fn check_type_argument_constraint(
    namespace: &mut TypeNamespace,
    arena: &mut TypeArena,
    parameter: (TypeParameterId, &str),
    argument: TypeId,
    bindings: &[(TypeParameterId, TypeId)],
    span: oxc_span::Span,
) {
    let (parameter_id, parameter_name) = parameter;
    let Some(constraint) = namespace.type_param_constraint(arena, parameter_id) else {
        return;
    };
    let constraint = crate::semantic::substitute_type_params(arena, constraint, bindings);
    if crate::semantic::contains_type_param(arena, constraint) {
        return;
    }
    if !crate::subtyping::is_subtype(arena, argument, constraint) {
        namespace.note_constraint_violation(parameter_name, argument, constraint, span);
    }
}

// TypeScript's own built-in generics that this checker gives at least a
// partial, honest representation rather than leaving totally unresolvable --
// which, before Void got the same treatment, could take an entire enclosing
// interface or function down with it through a single `?`.
//
// `Array<T>`/`ReadonlyArray<T>` map onto this checker's existing array type
// directly, the same as `T[]`, including propagating a failure to resolve
// their own argument the same way `T[]` does. tsc actually requires the
// argument here (TS2314) rather than defaulting a bare `Array` to
// `Array<any>`, so a missing argument is treated the same as an unresolvable
// one -- this checker doesn't raise its own arity diagnostic for a built-in
// (only for a user's own generic), but it won't silently invent `any` either.
//
// `Promise<T>` has no await/`.then()` modeling at all yet, so it resolves to
// an opaque, empty, named object: honest about what isn't checked (member
// access on it correctly reports "does not exist") without blocking
// everything around it. Unlike Array, a bad argument doesn't fail the whole
// type here -- nothing structurally depends on it being right, since nothing
// looks inside a Promise anyway, so it's treated the same lenient way an
// unresolvable explicit argument is treated for a user-defined generic
// elsewhere in this file.
//
// `Record<K, V>` gets the same opaque-object treatment as Promise, plus one
// more thing Promise doesn't need: its value type V is recorded on the
// application (see TypeArena::record_value_type) so a later property access
// can return V instead of "does not exist". This does not model K at all --
// any key is accepted, not just ones K would allow -- so it under-checks
// rather than risking a false positive on a key it can't classify. `Partial`
// and other mapped types need real mapped-type support this checker doesn't
// have yet, and are deliberately left alone; a plain `Partial<T>` still
// resolves to nothing.
fn resolve_builtin_generic(
    name: &str,
    reference: &oxc_ast::ast::TSTypeReference,
    namespace: &mut TypeNamespace,
    arena: &mut TypeArena,
) -> Option<TypeId> {
    let first_type_argument = |namespace: &mut TypeNamespace, arena: &mut TypeArena| {
        reference
            .type_arguments
            .as_ref()
            .and_then(|arguments| arguments.params.first())
            .map(|argument| resolve_ts_type(argument, namespace, arena))
    };

    match name {
        "Array" | "ReadonlyArray" => {
            let element = first_type_argument(namespace, arena)??;
            Some(arena.alloc(Type::Array(element)))
        }
        "Promise" => {
            let argument = match first_type_argument(namespace, arena) {
                Some(resolved) => resolved.unwrap_or_else(|| arena.error()),
                None => arena.void(),
            };
            // Every Promise<T> has the same empty body and is told apart by its argument,
            // which the application keeps: Promise<number> and Promise<string> are two
            // ids, and two mentions of Promise<number> are one.
            let body = arena.alloc(Type::Object(ObjectType::new(Vec::new())));
            let slot = arena.builtin_slot("Promise");
            Some(arena.alloc_app(slot, vec![argument], body))
        }
        "Record" => {
            let key_argument = first_type_argument(namespace, arena)
                .map(|resolved| resolved.unwrap_or_else(|| arena.error()));
            let value_argument = reference
                .type_arguments
                .as_ref()
                .and_then(|arguments| arguments.params.get(1))
                .map(|argument| {
                    resolve_ts_type(argument, namespace, arena).unwrap_or_else(|| arena.error())
                });
            let (Some(key), Some(value)) = (key_argument, value_argument) else {
                // Fewer than two arguments: not a well-formed Record. Left
                // unresolvable rather than guessing at a key or value type
                // that was never given.
                return None;
            };
            // Same shape as Promise above. The value type is not stored anywhere else:
            // TypeArena::record_value_type reads it back from the application's second
            // argument, so Record<string, number> and Record<string, string> each
            // carry their own.
            let body = arena.alloc(Type::Object(ObjectType::new(Vec::new())));
            let slot = arena.builtin_slot("Record");
            Some(arena.alloc_app(slot, vec![key, value], body))
        }
        _ => None,
    }
}

fn resolve_literal_type(
    literal: &oxc_ast::ast::TSLiteral,
    arena: &mut TypeArena,
) -> Option<TypeId> {
    use oxc_ast::ast::TSLiteral;
    match literal {
        TSLiteral::StringLiteral(s) => Some(arena.alloc(Type::StringLiteral(s.value.to_string()))),
        TSLiteral::NumericLiteral(n) => Some(arena.alloc(Type::NumberLiteral(n.value))),
        TSLiteral::BooleanLiteral(b) => Some(arena.alloc(Type::BooleanLiteral(b.value))),

        _ => None,
    }
}

pub fn resolve_function_params(
    params: &FormalParameters,
    namespace: &mut TypeNamespace,
    arena: &mut TypeArena,
) -> Option<Vec<Param>> {
    let mut resolved = Vec::with_capacity(params.items.len() + 1);
    for (param, optional) in params.items.iter().zip(optional_flags(params)) {
        let annotation = param.type_annotation.as_ref()?;
        let type_id = resolve_type_annotation(annotation, namespace, arena)?;
        resolved.push(Param {
            type_id,
            optional,
            rest: false,
            name: binding_name(&param.pattern),
        });
    }

    if let Some(rest) = &params.rest {
        let annotation = rest.type_annotation.as_ref()?;
        let type_id = resolve_type_annotation(annotation, namespace, arena)?;
        resolved.push(Param {
            type_id,
            optional: false,
            rest: true,
            name: binding_name(&rest.rest.argument),
        });
    }

    Some(resolved)
}

// Whether each parameter can be left out of a call. `x?: T` always can. A default
// value (`x: T = v`) can too, but only when no required parameter follows it: in
// `(a = 1, b: number)` a caller has to supply `a` to reach `b`, so tsc counts both
// as required. Walking from the end is what turns "a required parameter follows"
// into one running flag.
//
// A default lives in FormalParameter::initializer. oxc only wraps a pattern in
// BindingPattern::AssignmentPattern for a default nested inside a destructuring
// pattern, so looking at the pattern alone missed every ordinary default and left
// `(name: string, greeting: string = "hi")` demanding both arguments. Both forms
// are honored here.
fn optional_flags(params: &FormalParameters) -> Vec<bool> {
    let mut required_follows = false;
    let mut flags: Vec<bool> = params
        .items
        .iter()
        .rev()
        .map(|param| {
            let has_default = param.initializer.is_some()
                || matches!(param.pattern, BindingPattern::AssignmentPattern(_));
            let optional = param.optional || (has_default && !required_follows);
            if !optional {
                required_follows = true;
            }
            optional
        })
        .collect();
    flags.reverse();
    flags
}

// None for a destructured pattern, which has no single name -- treated as
// unnamed rather than guessed.
pub(crate) fn binding_name(pattern: &BindingPattern) -> Option<std::rc::Rc<str>> {
    match pattern {
        BindingPattern::BindingIdentifier(id) => Some(id.name.as_str().into()),
        BindingPattern::AssignmentPattern(assignment) => binding_name(&assignment.left),
        BindingPattern::ObjectPattern(_) | BindingPattern::ArrayPattern(_) => None,
    }
}

pub fn resolve_params_with_any_fallback(
    params: &FormalParameters,
    namespace: &mut TypeNamespace,
    arena: &mut TypeArena,
) -> Vec<Param> {
    let mut resolved: Vec<Param> = params
        .items
        .iter()
        .zip(optional_flags(params))
        .map(|(param, optional)| {
            let type_id = param
                .type_annotation
                .as_ref()
                .and_then(|annotation| resolve_type_annotation(annotation, namespace, arena))
                .unwrap_or_else(|| arena.any());
            Param {
                type_id,
                optional,
                rest: false,
                name: binding_name(&param.pattern),
            }
        })
        .collect();

    if let Some(rest) = &params.rest {
        let type_id = rest
            .type_annotation
            .as_ref()
            .and_then(|annotation| resolve_type_annotation(annotation, namespace, arena))
            .unwrap_or_else(|| {
                let any = arena.any();
                arena.alloc(Type::Array(any))
            });
        resolved.push(Param {
            type_id,
            optional: false,
            rest: true,
            name: binding_name(&rest.rest.argument),
        });
    }

    resolved
}

pub fn resolve_object_members(
    // References rather than owned signatures: interface declaration merging
    // (see merged_interface_parts in namespace.rs) needs to resolve members
    // gathered from more than one TSInterfaceDeclaration's own Vec together as
    // one shape, so this takes a caller-built slice of references rather than
    // borrowing one Vec's storage directly. TSSignature has no plain Clone
    // (only oxc's arena-allocating CloneIn), so collecting owned copies instead
    // was not an option; a plain type literal, which only ever has the one Vec,
    // pays the small cost of collecting a Vec of references to its own members
    // just to match this shared shape.
    members: &[&TSSignature],
    namespace: &mut TypeNamespace,
    arena: &mut TypeArena,
) -> Option<TypeId> {
    let mut properties: Vec<PropertyEntry> = Vec::with_capacity(members.len());

    for member in members {
        // As with class members in namespace.rs, one member this checker cannot
        // represent, such as a call signature or an index signature inside an
        // interface, makes the whole interface unresolvable rather than silently
        // dropping just that member.
        let entry = match *member {
            TSSignature::TSPropertySignature(property) => {
                let PropertyKey::StaticIdentifier(key) = &property.key else {
                    return None;
                };
                let annotation = property.type_annotation.as_ref()?;
                let type_id = resolve_type_annotation(annotation, namespace, arena)?;
                PropertyEntry {
                    name: key.name.to_string().into(),
                    type_id,
                    optional: property.optional,
                    is_method: false,
                }
            }
            TSSignature::TSMethodSignature(method) => {
                resolve_method_signature(method, namespace, arena)?
            }
            _ => return None,
        };

        // ObjectType's merge-join over sorted property lists gives wrong answers
        // when a name appears twice, and two method signatures with one name are an
        // overload set, which has no representation here. Either way the
        // declaration is not modelled, so it stays unresolvable.
        if properties
            .iter()
            .any(|existing| existing.name == entry.name)
        {
            return None;
        }
        properties.push(entry);
    }

    Some(arena.alloc(Type::Object(ObjectType::new(properties))))
}

// A method signature (`get(name: string): T`, `map<U>(f: (x: T) => U): U[]`) is
// a property whose type is a function, flagged as declared with method syntax so
// its parameters are compared bivariantly (see subtyping::property_is_subtype).
//
// Left unresolvable, exactly like a class method, when any part cannot be
// modelled: an accessor (`get x(): T`), a computed or non-identifier key, an
// explicit `this` parameter, or no return type annotation. Parameters without an
// annotation are not one of those: they become `any` and are reported as implicit
// any, the way a function type's parameters are.
//
// The method's own type parameters are in scope only while its signature is
// resolved, so `U` never leaks into the enclosing interface.
fn resolve_method_signature(
    method: &TSMethodSignature,
    namespace: &mut TypeNamespace,
    arena: &mut TypeArena,
) -> Option<PropertyEntry> {
    if !matches!(method.kind, TSMethodSignatureKind::Method)
        || method.computed
        || method.this_param.is_some()
    {
        return None;
    }
    let PropertyKey::StaticIdentifier(key) = &method.key else {
        return None;
    };
    let return_annotation = method.return_type.as_ref()?;

    let scope = namespace.push_decl_type_params(arena, method.type_parameters.as_deref());
    namespace.note_implicit_any_params(&method.params);
    let params = resolve_params_with_any_fallback(&method.params, namespace, arena);
    let return_type = resolve_type_annotation(return_annotation, namespace, arena);
    let type_params =
        resolve_function_type_params(method.type_parameters.as_deref(), namespace, arena);
    namespace.pop_type_params(scope);

    let function = arena.alloc(Type::Function(FunctionType {
        params,
        return_type: return_type?,
        type_params,
        is_untyped: false,
    }));
    Some(PropertyEntry {
        name: key.name.to_string().into(),
        type_id: function,
        optional: method.optional,
        is_method: true,
    })
}

struct DottedName<'n> {
    name: &'n str,
}

// `A.B.C` as one string, or None for a name that is not a plain dotted path.
fn dotted_type_name(name: &TSTypeName) -> Option<String> {
    match name {
        TSTypeName::IdentifierReference(id) => Some(id.name.to_string()),
        TSTypeName::QualifiedName(qualified) => Some(format!(
            "{}.{}",
            dotted_type_name(&qualified.left)?,
            qualified.right.name
        )),
        _ => None,
    }
}
