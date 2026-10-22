// Step 6b: binding the names an `import` declaration brings in.
//
// Runs at the start of the declare pass, before the file's own declarations, so every
// later phase sees an imported name as it sees a local one: a type name resolves in the
// namespace, a value name has a type under its symbol. A specifier the project does not
// know, or a name the module does not export, binds nothing and the name stays unknown.
//
// Problem: that silence also hid a typo. `import { Usr } from "./a"` bound nothing and
// reported nothing, so a broken import looked clean.
// Picked: report only what is certain. A relative specifier no file answers is TS2307.
// A name missing from a module whose export list is complete is TS2305 (TS1192 for a
// default). Against a list that left something out (a class, `export *`, an inferred
// value) the name may be one we dropped, so nothing is reported.
// A package specifier ("react") is reported only when the source says it is certainly
// missing (the project driver does, when no configuration could be hiding it), and a name
// missing from a partial module is still silent.

use oxc_semantic::Scoping;
use oxc_span::{GetSpan, Span};

use crate::diagnostic_messages::messages;
use oxc_ast::ast::{
    ImportDeclaration, ImportDeclarationSpecifier, ImportOrExportKind, ModuleExportName, Program,
    Statement, TSModuleReference,
};

use crate::keys::{DeclKey, DeclKind};
use crate::portable::{DeclProvider, GlobalNamespace, ImportSource, ModuleExports};
use crate::types::{ObjectType, PropertyEntry, Type};

use super::context::CheckContext;

pub(super) fn bind_imports<'ast>(
    program: &'ast Program<'ast>,
    scoping: &Scoping,
    ctx: &mut CheckContext<'ast, '_>,
) {
    let Some(source) = ctx.imports else {
        return;
    };
    bind_project_globals(program, scoping, ctx, source);
    // UMD aliases are global names for script consumers. An external module is isolated
    // unless the compiler option allowUmdGlobalAccess is enabled (not modelled here), so
    // do not inject those globals into files that already have import/export syntax.
    let is_external_module = program.body.iter().any(|statement| {
        matches!(
            statement,
            Statement::ImportDeclaration(_)
                | Statement::ExportDeclaration(_)
                | Statement::ExportNamedDeclaration(_)
                | Statement::ExportDefaultDeclaration(_)
                | Statement::ExportAllDeclaration(_)
                | Statement::ExportFromDeclaration(_)
                | Statement::TSExportAssignment(_)
                | Statement::TSNamespaceExportDeclaration(_)
                | Statement::TSImportEqualsDeclaration(_)
        )
    });
    if !is_external_module {
        for global in source.global_namespaces() {
            bind_global_namespace(ctx, source, &global);
        }
    }
    for stmt in &program.body {
        match stmt {
            Statement::ImportDeclaration(decl) => {
                report_missing_module(source, decl.source.value.as_str(), decl.source.span, ctx);
                bind_declaration(decl, source, ctx);
            }
            Statement::ExportFromDeclaration(from) => {
                let specifier = from.source.value.as_str();
                report_missing_module(source, specifier, from.source.span, ctx);
                for spec in &from.specifiers {
                    let name = module_export_name(&spec.local);
                    report_missing_name(source, specifier, &name, spec.local.span(), ctx);
                }
            }
            Statement::ExportAllDeclaration(all) => {
                report_missing_module(source, all.source.value.as_str(), all.source.span, ctx);
            }
            // `import x = require(\"./a\")` is `import * as x` for a module with named
            // exports; for one that uses `export =` the list is partial and `x` stays
            // unbound.
            Statement::TSImportEqualsDeclaration(decl) => {
                if let TSModuleReference::ExternalModuleReference(reference) =
                    &decl.module_reference
                {
                    let specifier = reference.expression.value.as_str();
                    report_missing_module(source, specifier, reference.expression.span, ctx);
                    // A module that says `export = x` is that one thing; any other is the
                    // object of its named exports.
                    let whole = source.export_keys(specifier, "export=");
                    if !whole.is_empty() {
                        for key in whole {
                            bind_key(
                                ctx,
                                source,
                                key,
                                &decl.id,
                                decl.import_kind == ImportOrExportKind::Type,
                            );
                        }
                    } else if decl.import_kind != ImportOrExportKind::Type {
                        bind_namespace_object(ctx, source, specifier, &decl.id);
                    }
                }
            }
            _ => {}
        }
    }
}

// Problem: a type, function or variable that one file declares globally (an interface in a
// script file, or a member of `declare global { }`) is visible to every other file of the
// project, but a file only knew what it declared or imported, so each use was reported as
// "Cannot find name".
// Picked: bind a project global only when the file uses a name nothing in it declares,
// which oxc lists as the root scope's unresolved references. Importing every global into
// every file would cost the number of globals times the number of files; this costs the
// names a file actually leaves unresolved. The names are taken in sorted order, so what is
// bound does not depend on hash order.
// Cost: a name declared several times (an interface in two files) is merged by the same
// code that merges a module augmentation, so it inherits that code's limits (a generic
// interface with a different arity keeps the first declaration).
fn bind_project_globals<'ast>(
    program: &'ast Program<'ast>,
    scoping: &Scoping,
    ctx: &mut CheckContext<'ast, '_>,
    source: &dyn ImportSource,
) {
    let mut names: Vec<String> = scoping
        .root_unresolved_references()
        .keys()
        .map(|name| name.to_string())
        .collect();
    names.sort();
    let own = super::declare::own_global_block_names(program);
    for name in names {
        if own.contains(&name) {
            continue;
        }
        // A name the standard library provides is bound to nothing here: a project file that
        // only adds members to it (`interface Window { app: App }`) would otherwise stand in
        // for the whole type and make every real member look missing.
        if crate::lib_global_types::is_lib_global_type(&name) {
            continue;
        }
        let keys = source.global_decls(&name);
        if keys.is_empty() {
            if source.global_name_unmodelled(&name) {
                ctx.namespace.mark_unmodelled_global(&name);
            }
            continue;
        }
        bind_global_name(ctx, source, &name, &keys);
    }
}

// Binds the declarations of one global name: its types go into the type namespace, its
// value (a variable, a function and its overloads, an enum) into `global_values`.
fn bind_global_name<'ast>(
    ctx: &mut CheckContext<'ast, '_>,
    source: &dyn ImportSource,
    name: &str,
    keys: &[DeclKey],
) {
    let provider: &dyn DeclProvider = source;
    let mut value: Option<crate::arena::TypeId> = None;
    let mut functions: Vec<crate::arena::TypeId> = Vec::new();
    for &key in keys {
        let Some(decl) = provider.decl(key) else {
            continue;
        };
        match decl.kind {
            DeclKind::Interface | DeclKind::Class | DeclKind::Alias => {
                bind_key_as(ctx, source, key, name, None, false);
            }
            DeclKind::Enum => {
                bind_key_as(ctx, source, key, name, None, false);
                if value.is_none()
                    && let Some((_, enum_value)) =
                        ctx.importer.import_enum(&mut ctx.arena, provider, key)
                {
                    value = Some(enum_value);
                }
            }
            DeclKind::Var => {
                if value.is_none() {
                    value = ctx.importer.import_decl(&mut ctx.arena, provider, key);
                }
            }
            DeclKind::Function => {
                if let Some(function) = ctx.importer.import_decl(&mut ctx.arena, provider, key) {
                    functions.push(function);
                }
            }
            DeclKind::Namespace => {}
        }
    }
    let value = match (value, functions.len()) {
        (Some(value), _) => Some(value),
        (None, 0) => None,
        (None, 1) => functions.pop(),
        (None, _) => {
            let first = functions[0];
            Some(ctx.arena.alloc_intersection(functions).unwrap_or(first))
        }
    };
    if let Some(value) = value {
        ctx.global_values.insert(name.to_string(), value);
    }
}

fn bind_global_namespace<'ast>(
    ctx: &mut CheckContext<'ast, '_>,
    source: &dyn ImportSource,
    global: &GlobalNamespace,
) {
    let provider: &dyn DeclProvider = source;
    let prefix = format!("{}.", global.name);
    let mut properties = Vec::new();
    let mut seen_values = std::collections::HashSet::new();

    for (name, key) in &global.members {
        let Some(decl) = provider.decl(*key) else {
            continue;
        };
        match decl.kind {
            DeclKind::Interface | DeclKind::Class | DeclKind::Alias | DeclKind::Enum => {
                let qualified = format!("{prefix}{name}");
                bind_key_as(ctx, source, *key, &qualified, None, false);
            }
            DeclKind::Function | DeclKind::Var | DeclKind::Namespace => {
                if !seen_values.insert(name.clone()) {
                    continue;
                }
                let value_type = match decl.kind {
                    DeclKind::Function | DeclKind::Var => {
                        ctx.importer.import_decl(&mut ctx.arena, provider, *key)
                    }
                    DeclKind::Namespace => None,
                    _ => None,
                };
                if let Some(type_id) = value_type {
                    properties.push(PropertyEntry {
                        name: name.as_str().into(),
                        type_id,
                        optional: false,
                        is_method: false,
                    });
                }
            }
        }
        // An enum has both a type-side declaration and a value-side object. The latter
        // is represented by the same portable enum declaration and is rebuilt here.
        if decl.kind == DeclKind::Enum && seen_values.insert(name.clone()) {
            if let Some((_, type_id)) = ctx.importer.import_enum(&mut ctx.arena, provider, *key) {
                properties.push(PropertyEntry {
                    name: name.as_str().into(),
                    type_id,
                    optional: false,
                    is_method: false,
                });
            }
        }
    }

    let namespace_shape = ctx.arena.alloc(Type::Object(ObjectType::new(properties)));
    let assignment_value = global
        .export_assignment
        .and_then(|key| ctx.importer.import_decl(&mut ctx.arena, provider, key));
    let empty_namespace = matches!(
        ctx.arena.get(namespace_shape),
        Type::Object(object) if object.properties.is_empty()
    );
    let value = match assignment_value {
        Some(assigned)
            if empty_namespace || ctx.arena.structurally_equal(assigned, namespace_shape) =>
        {
            assigned
        }
        Some(assigned) => ctx
            .arena
            .alloc_intersection(vec![assigned, namespace_shape])
            .unwrap_or(assigned),
        None => namespace_shape,
    };
    ctx.global_values.insert(global.name.clone(), value);
}

fn module_export_name(name: &ModuleExportName<'_>) -> String {
    match name {
        ModuleExportName::IdentifierName(n) => n.name.to_string(),
        ModuleExportName::IdentifierReference(n) => n.name.to_string(),
        ModuleExportName::StringLiteral(s) => s.value.to_string(),
    }
}

// TS2307, for a relative specifier that no file of the project answers.
fn report_missing_module<'ast>(
    source: &dyn ImportSource,
    specifier: &str,
    span: Span,
    ctx: &mut CheckContext<'ast, '_>,
) {
    if source.module_exports(specifier) == ModuleExports::Unresolved {
        ctx.error(messages::module_not_found(specifier), span);
    }
}

// TS2305 (TS1192 for `default`), for a name a complete module does not export.
fn report_missing_name<'ast>(
    source: &dyn ImportSource,
    specifier: &str,
    name: &str,
    span: Span,
    ctx: &mut CheckContext<'ast, '_>,
) {
    let ModuleExports::Known { complete: true } = source.module_exports(specifier) else {
        return;
    };
    if !source.export_keys(specifier, name).is_empty() {
        return;
    }
    let message = if name == "default" {
        messages::no_default_export(specifier)
    } else {
        messages::no_exported_member(specifier, name)
    };
    ctx.error(message, span);
}

fn bind_declaration<'ast>(
    decl: &ImportDeclaration<'_>,
    source: &dyn ImportSource,
    ctx: &mut CheckContext<'ast, '_>,
) {
    let specifier = decl.source.value.as_str();
    let whole_declaration_is_type = decl.import_kind == ImportOrExportKind::Type;
    for item in decl.specifiers.iter().flatten() {
        match item {
            ImportDeclarationSpecifier::ImportSpecifier(spec) => {
                let imported = match &spec.imported {
                    ModuleExportName::IdentifierName(n) => n.name.as_str(),
                    ModuleExportName::IdentifierReference(n) => n.name.as_str(),
                    ModuleExportName::StringLiteral(s) => s.value.as_str(),
                };
                let type_only =
                    whole_declaration_is_type || spec.import_kind == ImportOrExportKind::Type;
                report_missing_name(source, specifier, imported, spec.imported.span(), ctx);
                bind_name(ctx, source, specifier, imported, &spec.local, type_only);
            }
            ImportDeclarationSpecifier::ImportDefaultSpecifier(spec) => {
                report_missing_name(source, specifier, "default", spec.local.span, ctx);
                // A module with no default that says `export = x` is what a default
                // import gets (the interop every bundler and `esModuleInterop` give).
                let imported = if source.export_keys(specifier, "default").is_empty()
                    && !source.export_keys(specifier, "export=").is_empty()
                {
                    "export="
                } else {
                    "default"
                };
                bind_name(
                    ctx,
                    source,
                    specifier,
                    imported,
                    &spec.local,
                    whole_declaration_is_type,
                );
            }
            ImportDeclarationSpecifier::ImportNamespaceSpecifier(spec) => {
                if !whole_declaration_is_type {
                    bind_namespace_object(ctx, source, specifier, &spec.local);
                }
            }
        }
    }
}

// `import * as ns`: `ns` is an object whose properties are the module's value exports (a
// function, a variable, an enum, a class constructor). A type the module exports is not a
// property, and `ns.Name` in a type position is not read yet, so that annotation stays
// unresolved and says nothing.
//
// Problem: a property missing from the object would be reported as not existing, so the
// object is only built from a module whose export list is complete. Against a partial one
// `ns` stays unbound, which loses an error and never invents one.
fn bind_namespace_object<'ast>(
    ctx: &mut CheckContext<'ast, '_>,
    source: &dyn ImportSource,
    specifier: &str,
    local: &oxc_ast::ast::BindingIdentifier<'_>,
) {
    let provider: &dyn DeclProvider = source;
    // The types the module exports are named `ns.Type`. A type that is there is right
    // whether or not the list is whole, so this does not wait for the module to be complete.
    for (name, key) in source.export_all(specifier) {
        let Some(decl) = provider.decl(key) else {
            continue;
        };
        if matches!(
            decl.kind,
            DeclKind::Interface | DeclKind::Class | DeclKind::Alias | DeclKind::Enum
        ) {
            let qualified = format!("{}.{name}", local.name);
            bind_key_as(ctx, source, key, &qualified, None, false);
        }
    }
    if source.module_exports(specifier) != (ModuleExports::Known { complete: true }) {
        return;
    }
    let Some(symbol_id) = local.symbol_id.get() else {
        return;
    };
    let mut properties: Vec<PropertyEntry> = Vec::new();
    for (name, key) in source.export_all(specifier) {
        let Some(decl) = provider.decl(key) else {
            continue;
        };
        let type_id = match decl.kind {
            DeclKind::Function | DeclKind::Var => {
                ctx.importer.import_decl(&mut ctx.arena, provider, key)
            }
            DeclKind::Enum => ctx
                .importer
                .import_enum(&mut ctx.arena, provider, key)
                .map(|(_, value)| value),
            _ => None,
        };
        let Some(type_id) = type_id else {
            continue;
        };
        if properties.iter().any(|p| &*p.name == name.as_str()) {
            continue;
        }
        properties.push(PropertyEntry {
            name: name.into(),
            type_id,
            optional: false,
            is_method: false,
        });
    }
    let object = ctx.arena.alloc(Type::Object(ObjectType::new(properties)));
    ctx.symbols.declare(symbol_id, object);
}

// Binds `local` to every declaration the module exports under `imported`. A name that
// is both a type and a value (`interface Foo` and `const Foo`) is bound as both, as it
// is in the file that declares it.
fn bind_name<'ast>(
    ctx: &mut CheckContext<'ast, '_>,
    source: &dyn ImportSource,
    specifier: &str,
    imported: &str,
    local: &oxc_ast::ast::BindingIdentifier<'_>,
    type_only: bool,
) {
    for key in source.export_keys(specifier, imported) {
        bind_key(ctx, source, key, local, type_only);
    }
    // `import { N }` also brings the types written inside `namespace N`, so `N.I` resolves.
    let prefix = format!("{imported}.");
    for (name, key) in source.export_all(specifier) {
        if let Some(rest) = name.strip_prefix(&prefix) {
            let qualified = format!("{}.{rest}", local.name);
            bind_key_as(ctx, source, key, &qualified, None, type_only);
        }
    }
}

fn bind_key<'ast>(
    ctx: &mut CheckContext<'ast, '_>,
    source: &dyn ImportSource,
    key: DeclKey,
    local: &oxc_ast::ast::BindingIdentifier<'_>,
    type_only: bool,
) {
    bind_key_as(
        ctx,
        source,
        key,
        &local.name,
        local.symbol_id.get(),
        type_only,
    );
}

// Merge two imported interface/class object shapes. A module augmentation contributes
// properties that the original declaration did not have; the original declaration wins
// for a duplicate non-method member (which is separately an invalid declaration merge),
// while duplicate methods are represented as overload alternatives.
fn merge_imported_object_shapes(
    arena: &mut crate::arena::TypeArena,
    original: crate::arena::TypeId,
    addition: crate::arena::TypeId,
) -> Option<crate::arena::TypeId> {
    let crate::types::Type::Object(original) = arena.get(original).clone() else {
        return None;
    };
    let crate::types::Type::Object(addition) = arena.get(addition).clone() else {
        return None;
    };
    let mut properties = original.properties.to_vec();
    for added in addition.properties.iter() {
        if let Some(existing) = properties
            .iter_mut()
            .find(|property| property.name == added.name)
        {
            if existing.is_method
                && added.is_method
                && !arena.structurally_equal(existing.type_id, added.type_id)
            {
                existing.type_id = arena
                    .alloc_intersection(vec![existing.type_id, added.type_id])
                    .unwrap_or(existing.type_id);
            }
            continue;
        }
        properties.push(added.clone());
    }
    Some(
        arena.alloc(crate::types::Type::Object(crate::types::ObjectType::new(
            properties,
        ))),
    )
}

// Binds the declaration under `key` to `name`, with `symbol` as the value's symbol when it
// has one (a qualified name like `ns.User` has a type and no symbol).
fn bind_key_as<'ast>(
    ctx: &mut CheckContext<'ast, '_>,
    source: &dyn ImportSource,
    key: DeclKey,
    name: &str,
    symbol: Option<oxc_semantic::SymbolId>,
    type_only: bool,
) {
    let provider: &dyn DeclProvider = source;
    let previous_keys = ctx.imported_names.get(name).cloned().unwrap_or_default();
    let Some(decl) = provider.decl(key) else {
        return;
    };
    // Remembered before the kind is looked at, so that a name this file only passes on
    // (`import { X } from "./a"; export { X }`) still finds the declaration.
    if !(type_only && matches!(decl.kind, DeclKind::Function | DeclKind::Var)) {
        ctx.imported_names
            .entry(name.to_string())
            .or_default()
            .push(key);
    }
    match decl.kind {
        DeclKind::Interface | DeclKind::Class | DeclKind::Alias => {
            let Some(type_id) = ctx.importer.import_decl(&mut ctx.arena, provider, key) else {
                return;
            };
            // A generic one carries its parameters, so `Box<number>` in this file can
            // be instantiated without the declaration's AST.
            let params = ctx
                .importer
                .generic_params(&mut ctx.arena, provider, key)
                .unwrap_or_default();

            let mergeable = matches!(decl.kind, DeclKind::Interface | DeclKind::Class)
                && previous_keys.iter().any(|previous| {
                    provider.decl(*previous).is_some_and(|previous_decl| {
                        matches!(previous_decl.kind, DeclKind::Interface | DeclKind::Class)
                    })
                });
            let incoming_is_generic = !params.is_empty();
            let mut binding_type = type_id;
            let mut binding_params = params;
            if mergeable {
                let previous_generic = ctx.namespace.imported_generic(name);
                if let crate::namespace::Resolution::Resolved(previous_type) =
                    ctx.namespace.resolve(name, &mut ctx.arena)
                {
                    let merge_input = match (&previous_generic, &binding_params[..]) {
                        (Some(previous), current)
                            if previous.params.len() == current.len() && !current.is_empty() =>
                        {
                            // The declarations live in different modules, so their type
                            // parameters have different identities. Align parameters by
                            // position before merging the additional properties. Keep
                            // unbound method-level parameters intact.
                            let substitutions = current
                                .iter()
                                .zip(&previous.params)
                                .map(|(incoming, existing)| (incoming.id, existing.node))
                                .collect::<Vec<_>>();
                            crate::semantic::substitute_bound_type_params(
                                &mut ctx.arena,
                                type_id,
                                &substitutions,
                            )
                        }
                        (None, current) if current.is_empty() => type_id,
                        // A mismatched generic arity cannot be merged soundly. Preserve
                        // the first declaration instead of silently replacing its shape.
                        _ => previous_type,
                    };
                    if let Some(merged) =
                        merge_imported_object_shapes(&mut ctx.arena, previous_type, merge_input)
                    {
                        binding_type = merged;
                        if let Some(previous) = previous_generic {
                            binding_params = previous.params.clone();
                        } else if incoming_is_generic {
                            // This branch is only reached when there was no prior generic
                            // shape. Do not replace the existing declaration on mismatch.
                            binding_type = previous_type;
                            binding_params = Vec::new();
                        }
                    } else {
                        binding_type = previous_type;
                        binding_params = previous_generic
                            .map(|previous| previous.params.clone())
                            .unwrap_or_default();
                    }
                }
            }
            ctx.namespace
                .insert_imported(name, binding_type, binding_params);
        }
        // `import type { f }` brings in no value, only the right to name types.
        DeclKind::Function | DeclKind::Var if !type_only => {
            let Some(type_id) = ctx.importer.import_decl(&mut ctx.arena, provider, key) else {
                return;
            };
            if let Some(symbol_id) = symbol {
                if decl.kind == DeclKind::Function
                    && let Some(existing) = ctx.symbols.get(symbol_id)
                    && matches!(
                        ctx.arena.get(existing),
                        crate::types::Type::Function(_) | crate::types::Type::Intersection(_)
                    )
                    && matches!(ctx.arena.get(type_id), crate::types::Type::Function(_))
                {
                    let combined = ctx
                        .arena
                        .alloc_intersection(vec![existing, type_id])
                        .unwrap_or(type_id);
                    ctx.symbols.declare(symbol_id, combined);
                } else {
                    ctx.symbols.declare(symbol_id, type_id);
                }
            }
        }
        // An enum is a type and a value under one name; `import type` brings in the type.
        DeclKind::Enum => {
            let Some((type_position, value_type)) =
                ctx.importer.import_enum(&mut ctx.arena, provider, key)
            else {
                return;
            };
            ctx.namespace
                .insert_imported(name, type_position, Vec::new());
            if !type_only {
                if let Some(symbol_id) = symbol {
                    ctx.symbols.declare(symbol_id, value_type);
                }
            }
        }
        _ => {}
    }
}
