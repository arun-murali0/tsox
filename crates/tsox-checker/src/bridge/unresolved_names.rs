use oxc_ast::ast::{
    IdentifierReference, Program, TSClassImplements, TSImportEqualsDeclaration,
    TSInterfaceHeritage, TSModuleReference, TSNamespaceDeclaration, TSTypeName, TSTypeQuery,
    TSTypeQueryExprName, TSTypeReference,
};
use oxc_ast_visit::{Visit, walk};
use oxc_semantic::Scoping;

use crate::lib_global_types::is_lib_global_type;
use crate::namespace::TypeNamespace;

use super::context::CheckContext;
use super::narrow::resolve_symbol_id;

// Problem: an unknown name in a type annotation was reported only where a variable or a
// parameter happened to carry it. `type G = A & Missing`, `interface I { m: Missing }`,
// `function f(): Missing`, `class C { x: Missing }`, `Array<Missing>` and `A | Missing`
// in a parameter all passed in silence, or became a "could not be resolved" warning
// when something used them, where tsc says "Cannot find name 'Missing'." (TS2304)
// at the name.
// Picked: one pass over every type reference in the file, after the declarations are
// known, reporting each name that resolves to nothing: no symbol in scope (which covers
// type parameters, imports and nested declarations), no top-level type in the namespace,
// and (for configured projects) no selected global-library declaration. Standalone
// checks retain the historical built-in-name fallback. Each name is reported once, at
// its own position, so `Missing7 & Missing8` is two errors, as in tsc.
// Cost: a type reference written as a qualified name (`Intl.DateTimeFormat`) is left
// alone, since its first part is a namespace and not a type, and `typeof x` in a type
// is a value reference this pass does not look at.
pub fn report_unresolved_type_names<'ast>(
    program: &Program<'ast>,
    scoping: &Scoping,
    ctx: &mut CheckContext<'ast, '_>,
) {
    let mut walker = Walker { scoping, ctx };
    walker.visit_program(program);
}

// True when `id` names no type anywhere: not declared in scope or in the type namespace.
// Standalone and comparison-harness checks retain the historical registry fallback because
// they have no configured library graph to import from. In a configured project, selected
// standard-library declarations are registered in the namespace and unselected ones must be
// reported as missing, just like any other absent global type.
pub fn is_unresolved_type_name(
    id: &IdentifierReference,
    scoping: &Scoping,
    namespace: &TypeNamespace,
    allow_legacy_library_fallback: bool,
) -> bool {
    resolve_symbol_id(id, scoping).is_none()
        && !namespace.contains(&id.name)
        && !namespace.is_unmodelled_global(&id.name)
        && !(allow_legacy_library_fallback && is_lib_global_type(&id.name))
}

struct Walker<'s, 'c, 'ast, 'src> {
    scoping: &'s Scoping,
    ctx: &'c mut CheckContext<'ast, 'src>,
}

impl Walker<'_, '_, '_, '_> {
    // A value name that nothing declares: no symbol, no library or project global, and not a
    // type either (a type used as a value is its own diagnostic, reported where the value is
    // read). The same tests the expression pass makes, so a position it visits agrees with it.
    fn value_unresolved(&self, id: &IdentifierReference) -> bool {
        let ctx = &*self.ctx;
        let name = id.name.as_str();
        resolve_symbol_id(id, self.scoping).is_none()
            && !matches!(name, "undefined" | "arguments" | "globalThis")
            && !is_type_keyword(name)
            // The parser binds a reference only to a symbol with a value side, so a namespace
            // that holds only types, an enum member of another declaration of the enum, and
            // the local of an export specifier can all look unresolved. A name that nothing in
            // the file declares at all is the one that is really missing.
            && !self
                .scoping
                .symbol_ids()
                .any(|symbol| self.scoping.symbol_name(symbol) == name)
            && !ctx.type_only_value_names.contains(name)
            && !ctx.global_values.contains_key(name)
            && !(ctx
                .imports
                .is_none_or(|source| !source.requires_registered_global_values())
                && super::expressions::is_ambient_global(name))
            && !ctx.namespace.is_unmodelled_global(name)
            && !ctx.namespace.contains(name)
            && !super::expressions::is_library_type_only(name)
    }

    // Reported once per position: the expression pass has usually been here already.
    fn report_once(
        &mut self,
        message: crate::diagnostic_messages::DiagnosticMessage,
        id: &IdentifierReference,
    ) {
        let already = self
            .ctx
            .diagnostics
            .iter()
            .any(|d| d.start == id.span.start && d.code == message.code);
        if !already {
            self.ctx.error(message, id.span);
        }
    }

    fn report_value(&mut self, id: &IdentifierReference) {
        if self.value_unresolved(id) {
            let message =
                crate::diagnostic_messages::messages::unresolved_identifier_hint(&id.name)
                    .unwrap_or_else(|| {
                        crate::diagnostic_messages::messages::unresolved_identifier(&id.name)
                    });
            self.report_once(message, id);
        }
    }

    fn type_unresolved(&self, id: &IdentifierReference) -> bool {
        let allow_legacy = self
            .ctx
            .imports
            .is_none_or(|source| !source.requires_registered_global_types());
        is_unresolved_type_name(id, self.scoping, &self.ctx.namespace, allow_legacy)
    }

    // The first name of a qualified type name is a namespace.
    fn check_namespace_head(&mut self, name: &TSTypeName) {
        let mut head = name;
        while let TSTypeName::QualifiedName(qualified) = head {
            head = &qualified.left;
        }
        if let TSTypeName::IdentifierReference(id) = head
            && self.type_unresolved(id)
            && !self.ctx.global_values.contains_key(id.name.as_str())
            && self.value_unresolved(id)
        {
            self.report_once(
                crate::diagnostic_messages::messages::unresolved_namespace(&id.name),
                id,
            );
        }
    }
}

impl<'a> Visit<'a> for Walker<'_, '_, '_, '_> {
    // Problem: a second block of a namespace (`namespace A { export interface I {} }`
    // then `namespace A { function f(i: I) {} }`) reported `I` as missing. oxc keeps one
    // scope per block, so the name is not lexically in reach of the second block, and
    // the top-level lookup below finds only `A.I`, never `I`.
    // Picked: enter the namespace's scope in the type namespace while its body is walked,
    // the way the declare and check passes do. A name is then looked up as `A.I` first,
    // which finds what any block of `A` declared, and a dotted `namespace A.B` enters A
    // and then B.
    // Cost: none beyond a push and a pop per namespace.
    fn visit_ts_namespace_declaration(&mut self, it: &TSNamespaceDeclaration<'a>) {
        self.ctx.namespace.push_scope(&it.id.name);
        walk::walk_ts_namespace_declaration(self, it);
        self.ctx.namespace.pop_scope();
    }

    // A name outside every type position is a value read: it has to be declared, however
    // far the expression pass got (a default parameter, a class property initializer, an
    // enum member, a computed key and a heritage clause are places it does not look).
    fn visit_identifier_reference(&mut self, it: &IdentifierReference<'a>) {
        self.report_value(it);
    }

    // The names inside a type name are checked by the type reference that owns them; a
    // qualified one starts with a namespace.
    fn visit_ts_type_name(&mut self, it: &TSTypeName<'a>) {
        if let TSTypeName::QualifiedName(_) = it {
            self.check_namespace_head(it);
        }
    }

    // `typeof q` names a value.
    fn visit_ts_type_query(&mut self, it: &TSTypeQuery<'a>) {
        match &it.expr_name {
            TSTypeQueryExprName::IdentifierReference(id) => self.report_value(id),
            TSTypeQueryExprName::QualifiedName(qualified) => {
                let mut head = &qualified.left;
                while let TSTypeName::QualifiedName(inner) = head {
                    head = &inner.left;
                }
                if let TSTypeName::IdentifierReference(id) = head {
                    self.report_value(id);
                }
            }
            _ => {}
        }
        if let Some(arguments) = &it.type_arguments {
            self.visit_ts_type_parameter_instantiation(arguments);
        }
    }

    // `class C implements Missing`: a type name, in a clause the type-reference visit skips.
    fn visit_ts_class_implements(&mut self, it: &TSClassImplements<'a>) {
        match &it.expression {
            TSTypeName::IdentifierReference(id) => {
                if self.type_unresolved(id) && self.value_unresolved(id) {
                    let message =
                        crate::diagnostic_messages::messages::unresolved_identifier(&id.name);
                    self.report_once(message, id);
                }
            }
            name @ TSTypeName::QualifiedName(_) => self.check_namespace_head(name),
            _ => {}
        }
        if let Some(arguments) = &it.type_arguments {
            self.visit_ts_type_parameter_instantiation(arguments);
        }
    }

    // `interface I extends Missing`: the same, with a heritage name of its own.
    fn visit_ts_interface_heritage(&mut self, it: &TSInterfaceHeritage<'a>) {
        match &it.type_name {
            TSTypeName::IdentifierReference(id) => {
                if self.type_unresolved(id) && self.value_unresolved(id) {
                    let message =
                        crate::diagnostic_messages::messages::unresolved_identifier(&id.name);
                    self.report_once(message, id);
                }
            }
            name @ TSTypeName::QualifiedName(_) => self.check_namespace_head(name),
            _ => {}
        }
        if let Some(arguments) = &it.type_arguments {
            self.visit_ts_type_parameter_instantiation(arguments);
        }
    }

    // Names inside `with (o) { ... }` may be properties of `o`, and tsc reports none of them.
    fn visit_with_statement(&mut self, it: &oxc_ast::ast::WithStatement<'a>) {
        self.visit_expression(&it.object);
    }

    // `import a = Missing.b`
    fn visit_ts_import_equals_declaration(&mut self, it: &TSImportEqualsDeclaration<'a>) {
        match &it.module_reference {
            TSModuleReference::IdentifierReference(id) => {
                if self.type_unresolved(id) && self.value_unresolved(id) {
                    self.report_once(
                        crate::diagnostic_messages::messages::unresolved_namespace(&id.name),
                        id,
                    );
                }
            }
            TSModuleReference::QualifiedName(name) => {
                let mut head = &name.left;
                while let TSTypeName::QualifiedName(inner) = head {
                    head = &inner.left;
                }
                if let TSTypeName::IdentifierReference(id) = head
                    && self.type_unresolved(id)
                    && self.value_unresolved(id)
                {
                    self.report_once(
                        crate::diagnostic_messages::messages::unresolved_namespace(&id.name),
                        id,
                    );
                }
            }
            _ => {}
        }
    }

    fn visit_ts_type_reference(&mut self, it: &TSTypeReference<'a>) {
        // `as const` parses as a reference to a type named `const`, which is a reserved word and
        // so never a declared name: it is the const assertion, not a missing type.
        let allow_legacy_library_fallback = self
            .ctx
            .imports
            .is_none_or(|source| !source.requires_registered_global_types());
        if let TSTypeName::IdentifierReference(id) = &it.type_name
            && id.name != "const"
            && is_unresolved_type_name(
                id,
                self.scoping,
                &self.ctx.namespace,
                allow_legacy_library_fallback,
            )
        {
            self.ctx.error(
                crate::diagnostic_messages::messages::unresolved_identifier(&id.name),
                id.span,
            );
        }
        // Type arguments (`Array<Missing>`) are references too.
        walk::walk_ts_type_reference(self, it);
    }
}

// The names of the primitive types: `class C implements number` parses them as type names, and
// tsc has other diagnostics for those.
fn is_type_keyword(name: &str) -> bool {
    matches!(
        name,
        "any"
            | "number"
            | "string"
            | "boolean"
            | "symbol"
            | "object"
            | "void"
            | "never"
            | "unknown"
            | "bigint"
            | "null"
            | "undefined"
    )
}
