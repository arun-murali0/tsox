use oxc_ast::ast::{
    IdentifierReference, Program, TSNamespaceDeclaration, TSTypeName, TSTypeReference,
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
