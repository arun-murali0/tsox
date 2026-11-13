// Syntactic checks of ambient and duplicate declarations, which need nothing but the file's
// own statements: the scope a `declare global` or `declare module "x"` sits in, what may
// appear inside an ambient context, and two bodies for one function.

use oxc_ast::ast::{
    BindingPattern, Class, ClassElement, Declaration, Function, Program, Statement,
    VariableDeclarationKind,
};
use oxc_span::GetSpan;

use super::context::CheckContext;
use super::declare::{TopLevelItem, items_in, namespace_members};
use super::exports::is_module_file;
use crate::diagnostic_messages::messages;

pub(super) fn check<'ast>(program: &'ast Program<'ast>, ctx: &mut CheckContext<'ast, '_>) {
    let is_module = is_module_file(program, &ctx.options);
    for stmt in &program.body {
        match stmt {
            // TS2669: `declare global` augments the global scope from a module; in a script
            // the file already declares at global scope.
            Statement::TSGlobalDeclaration(block) => {
                if !is_module {
                    ctx.error(messages::global_augmentation_outside_module(), block.span);
                }
                check_ambient_statements(&block.body.body, ctx);
            }
            // TS2436: only a script's `declare module "./x"` is an ambient declaration, and an
            // ambient module cannot be named relatively (in a module it is an augmentation of
            // the file `./x`, which may be).
            Statement::TSExternalModuleDeclaration(module) => {
                let specifier = module.id.value.as_str();
                if !is_module && (specifier.starts_with("./") || specifier.starts_with("../")) {
                    ctx.error(messages::ambient_module_relative_name(), module.id.span);
                }
                // TS2664: in a module the block augments the module it names, which has to
                // exist. A pattern (`*.css`) names no single module and is left alone.
                if is_module && !specifier.contains('*') {
                    let found = ctx.imports.is_some_and(|imports| {
                        imports.module_exports(specifier)
                            != crate::portable::ModuleExports::Unresolved
                    });
                    if !found {
                        ctx.error(
                            messages::invalid_module_name_in_augmentation(specifier),
                            module.id.span,
                        );
                    }
                }
                if let Some(body) = &module.body {
                    check_ambient_statements(&body.body, ctx);
                }
            }
            Statement::ClassDeclaration(class) => check_class(class, class.declare, ctx),
            Statement::VariableDeclaration(decl) if decl.declare => {
                check_ambient_variable(decl, ctx)
            }
            Statement::FunctionDeclaration(func) if func.declare => {
                check_ambient_function(func, ctx)
            }
            Statement::TSNamespaceDeclaration(ns) if ns.declare => {
                check_ambient_items(namespace_members(ns), ctx);
            }
            Statement::ExportDeclaration(export) => match &export.declaration {
                Declaration::VariableDeclaration(decl) if decl.declare => {
                    check_ambient_variable(decl, ctx)
                }
                Declaration::FunctionDeclaration(func) if func.declare => {
                    check_ambient_function(func, ctx)
                }
                Declaration::TSNamespaceDeclaration(ns) if ns.declare => {
                    check_ambient_items(namespace_members(ns), ctx)
                }
                _ => {}
            },
            _ => {}
        }
    }
    check_duplicate_implementations(&program.body, ctx);
}

// Everything inside `declare global { }`, `declare module "x" { }` and `declare namespace N { }`
// is ambient, whether or not each member repeats `declare`.
fn check_ambient_statements<'ast>(
    statements: &'ast [Statement<'ast>],
    ctx: &mut CheckContext<'ast, '_>,
) {
    check_ambient_items(items_in(statements), ctx);
}

fn check_ambient_items<'ast>(
    items: Vec<(TopLevelItem<'ast>, bool)>,
    ctx: &mut CheckContext<'ast, '_>,
) {
    for (item, _) in items {
        match item {
            TopLevelItem::Variable(decl) => check_ambient_variable(decl, ctx),
            TopLevelItem::Function(func) => check_ambient_function(func, ctx),
            TopLevelItem::Class(class) => check_class(class, true, ctx),
            TopLevelItem::Namespace(ns) => check_ambient_items(namespace_members(ns), ctx),
            TopLevelItem::ExternalModule(module) => {
                if let Some(body) = &module.body {
                    check_ambient_statements(&body.body, ctx);
                }
            }
            _ => {}
        }
    }
}

// TS1039. A `const` may keep a literal initializer in an ambient context (`declare const a = 1`),
// so only `var` and `let` are reported here.
fn check_ambient_variable<'ast>(
    decl: &'ast oxc_ast::ast::VariableDeclaration<'ast>,
    ctx: &mut CheckContext<'ast, '_>,
) {
    if matches!(decl.kind, VariableDeclarationKind::Const) {
        return;
    }
    for declarator in &decl.declarations {
        if let Some(init) = &declarator.init {
            if matches!(declarator.id, BindingPattern::BindingIdentifier(_)) {
                ctx.error(messages::ambient_initializer(), init.span());
            }
        }
    }
}

// TS1039 and TS1183 inside a class: a `declare` field keeps no initializer anywhere, and in an
// ambient class (`declare class`, or any class inside an ambient context) neither a field
// initializer nor a method body is allowed.
fn check_class<'ast>(class: &'ast Class<'ast>, ambient: bool, ctx: &mut CheckContext<'ast, '_>) {
    for element in &class.body.body {
        match element {
            ClassElement::PropertyDefinition(prop) if prop.declare || ambient => {
                if let Some(value) = &prop.value {
                    ctx.error(messages::ambient_initializer(), value.span());
                }
            }
            ClassElement::MethodDefinition(method) if ambient && method.value.body.is_some() => {
                ctx.error(messages::ambient_implementation(), method.key.span());
            }
            _ => {}
        }
    }
}

// TS1183.
fn check_ambient_function<'ast>(func: &'ast Function<'ast>, ctx: &mut CheckContext<'ast, '_>) {
    if func.body.is_some() {
        let span = func.id.as_ref().map_or(func.span, |id| id.span);
        ctx.error(messages::ambient_implementation(), span);
    }
}

// TS2393: two bodies for one function name in the same scope. Overload signatures (no body) and
// ambient declarations do not count, and a name declared in two different scopes is fine.
fn check_duplicate_implementations<'ast>(
    statements: &'ast [Statement<'ast>],
    ctx: &mut CheckContext<'ast, '_>,
) {
    let mut seen: std::collections::BTreeMap<String, Vec<oxc_span::Span>> = Default::default();
    for stmt in statements {
        let func = match stmt {
            Statement::FunctionDeclaration(func) => Some(&**func),
            Statement::ExportDeclaration(export) => match &export.declaration {
                Declaration::FunctionDeclaration(func) => Some(&**func),
                _ => None,
            },
            _ => None,
        };
        let Some(func) = func else { continue };
        if func.declare || func.body.is_none() {
            continue;
        }
        if let Some(id) = &func.id {
            seen.entry(id.name.to_string()).or_default().push(id.span);
        }
    }
    for spans in seen.into_values().filter(|spans| spans.len() > 1) {
        for span in spans {
            ctx.error(messages::duplicate_function_implementation(), span);
        }
    }
}
