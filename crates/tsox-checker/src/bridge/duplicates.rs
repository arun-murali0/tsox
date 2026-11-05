// Duplicate declarations in one scope, by the rules of tsc's binder.
//
// Every declaration adds a meaning to a name's symbol: a `var` is a function-scoped
// variable, a `let` a block-scoped one, and a class, a function, an interface, an alias,
// an enum and a namespace each have their own. Each meaning lists the meanings it
// cannot share a name with (its "excludes"), and a declaration that meets one of them
// is reported, together with every earlier declaration of that name, as TS2300, or as
// TS2451 when the earlier symbol is a block-scoped variable. The merges that are allowed
// (an interface with a class, a function with a namespace, two interfaces) fall out of
// the same table.
//
// The type-only pairs inside a file's top level are reported by the declare pass too, so
// this pass skips a position that already has a duplicate report.

use oxc_ast::ast::{
    ArrowFunctionExpression, BindingPattern, BlockStatement, Class, ClassType, ForInStatement,
    ForOfStatement, ForStatement, Function, FunctionType, ImportDeclaration,
    ImportDeclarationSpecifier, Program, Statement, StaticBlock, SwitchStatement,
    TSEnumDeclaration, TSEnumMemberName, TSExportAssignment, TSExternalModuleDeclaration,
    TSImportEqualsDeclaration, TSInterfaceDeclaration, TSModuleBlock, TSNamespaceDeclaration,
    TSNamespaceDeclarationBody, TSTypeAliasDeclaration, VariableDeclaration,
    VariableDeclarationKind,
};
use oxc_ast_visit::{Visit, walk};
use oxc_semantic::ScopeFlags;
use oxc_span::{GetSpan, Span};
use std::collections::{BTreeMap, HashMap};

use crate::diagnostic_codes::DiagnosticCode;
use crate::diagnostic_messages::messages;

use super::context::CheckContext;
use super::declare::{TopLevelItem, items_in, namespace_members};

const FUNCTION_SCOPED: u32 = 1;
const BLOCK_SCOPED: u32 = 2;
const FUNCTION: u32 = 4;
const CLASS: u32 = 8;
const INTERFACE: u32 = 16;
const CONST_ENUM: u32 = 32;
const REGULAR_ENUM: u32 = 64;
const VALUE_MODULE: u32 = 128;
const NAMESPACE_MODULE: u32 = 256;
const TYPE_ALIAS: u32 = 512;
const ALIAS: u32 = 1024;
const ENUM_MEMBER: u32 = 2048;

const ENUM: u32 = CONST_ENUM | REGULAR_ENUM;
const VALUE: u32 =
    FUNCTION_SCOPED | BLOCK_SCOPED | FUNCTION | CLASS | ENUM | VALUE_MODULE | ENUM_MEMBER;
const TYPE: u32 = CLASS | INTERFACE | ENUM | TYPE_ALIAS | ENUM_MEMBER;

fn excludes(flags: u32) -> u32 {
    match flags {
        FUNCTION_SCOPED => VALUE & !FUNCTION_SCOPED,
        BLOCK_SCOPED => VALUE,
        FUNCTION => VALUE & !(FUNCTION | VALUE_MODULE | CLASS),
        CLASS => (VALUE | TYPE) & !(VALUE_MODULE | INTERFACE | FUNCTION),
        INTERFACE => TYPE & !(INTERFACE | CLASS),
        REGULAR_ENUM => (VALUE | TYPE) & !(REGULAR_ENUM | VALUE_MODULE),
        CONST_ENUM => (VALUE | TYPE) & !(CONST_ENUM | VALUE_MODULE),
        VALUE_MODULE => VALUE & !(FUNCTION | CLASS | REGULAR_ENUM | VALUE_MODULE),
        NAMESPACE_MODULE => 0,
        TYPE_ALIAS => TYPE,
        ALIAS => ALIAS,
        ENUM_MEMBER => VALUE | TYPE,
        _ => 0,
    }
}

#[derive(Default)]
struct Symbol {
    flags: u32,
    declarations: Vec<Span>,
    // The members of an enum, which merge across its declarations.
    members: HashMap<String, Symbol>,
}

struct Scope {
    names: HashMap<String, Symbol>,
    function_level: bool,
}

struct Found {
    name: String,
    span: Span,
    block_scoped: bool,
    enum_merge: bool,
}

pub fn report_duplicate_declarations<'ast>(
    program: &Program<'ast>,
    ctx: &mut CheckContext<'ast, '_>,
) {
    let mut walker = Walker {
        scopes: Vec::new(),
        found: Vec::new(),
    };
    walker.visit_program(program);
    // Exported declarations inside separate blocks of one namespace share one
    // symbol table. The ordinary AST walk deliberately gives each block its own
    // scope so private members remain block-local; this second pass joins only the
    // exported members, without changing that private-member behavior.
    report_cross_block_namespace_duplicates(&items_in(&program.body), &mut walker.found);
    let mut seen: Vec<u32> = ctx
        .diagnostics
        .iter()
        .filter(|d| {
            matches!(
                d.code,
                DiagnosticCode::DuplicateTypeDeclaration
                    | DiagnosticCode::DuplicateBlockScopedVariable
                    | DiagnosticCode::EnumDeclarationMerge
            )
        })
        .map(|d| d.start)
        .collect();
    for found in walker.found {
        if seen.contains(&found.span.start) {
            continue;
        }
        seen.push(found.span.start);
        let message = if found.enum_merge {
            messages::enum_declaration_merge()
        } else if found.block_scoped {
            messages::duplicate_block_scoped_variable(&found.name)
        } else {
            messages::duplicate_type_declaration(&found.name)
        };
        ctx.error(message, found.span);
    }

    // These diagnostics concern pairs that are legal to merge structurally but
    // have an invalid export boundary or declaration order. They are deliberately
    // checked after the binder-style duplicate pass, so a namespace declaration
    // can still contribute its members and produce useful secondary diagnostics.
    let items = items_in(&program.body);
    report_namespace_export_mismatches(&items, ctx);
    report_namespace_merge_order(&items, ctx);
    report_enum_merge_initializers(&items, ctx);
}

struct Walker {
    scopes: Vec<Scope>,
    found: Vec<Found>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MergeKind {
    Alias,
    Interface,
    Class,
    Enum,
    Function,
    Namespace,
    Variable,
}

struct NamedDeclaration {
    name: String,
    exported: bool,
    span: Span,
    kind: MergeKind,
}

fn named_declarations(items: &[(TopLevelItem<'_>, bool)]) -> Vec<NamedDeclaration> {
    let mut declarations = Vec::new();
    for (item, exported) in items.iter().copied() {
        let mut push = |name: &str, span: Span, kind| {
            declarations.push(NamedDeclaration {
                name: name.to_string(),
                exported,
                span,
                kind,
            });
        };
        match item {
            TopLevelItem::Alias(decl) => {
                push(&decl.id.name, decl.id.span, MergeKind::Alias);
            }
            TopLevelItem::Interface(decl) => {
                push(&decl.id.name, decl.id.span, MergeKind::Interface);
            }
            TopLevelItem::Class(class) => {
                if let Some(id) = &class.id {
                    push(&id.name, id.span, MergeKind::Class);
                }
            }
            TopLevelItem::Enum(decl) => {
                push(&decl.id.name, decl.id.span, MergeKind::Enum);
            }
            TopLevelItem::Function(func) => {
                if let Some(id) = &func.id {
                    push(&id.name, id.span, MergeKind::Function);
                }
            }
            TopLevelItem::Namespace(namespace) => {
                push(&namespace.id.name, namespace.id.span, MergeKind::Namespace);
            }
            TopLevelItem::Variable(decl) => {
                for declarator in &decl.declarations {
                    let mut bindings = Vec::new();
                    binding_names(&declarator.id, &mut bindings);
                    for (name, span) in bindings {
                        push(&name, span, MergeKind::Variable);
                    }
                }
            }
            TopLevelItem::ExternalModule(_) => {}
        }
    }
    declarations
}

fn binding_names(pattern: &BindingPattern<'_>, out: &mut Vec<(String, Span)>) {
    match pattern {
        BindingPattern::BindingIdentifier(id) => out.push((id.name.to_string(), id.span)),
        BindingPattern::ObjectPattern(object) => {
            for property in &object.properties {
                binding_names(&property.value, out);
            }
            if let Some(rest) = &object.rest {
                binding_names(&rest.argument, out);
            }
        }
        BindingPattern::ArrayPattern(array) => {
            for element in array.elements.iter().flatten() {
                binding_names(element, out);
            }
            if let Some(rest) = &array.rest {
                binding_names(&rest.argument, out);
            }
        }
        BindingPattern::AssignmentPattern(assign) => binding_names(&assign.left, out),
    }
}

// Only declaration pairs that share the same merge contract require a consistent
// export marker here. In particular, class/interface, enum/enum, and
// namespace/namespace declarations are checked; a namespace can coexist with a
// class, function, enum, or interface without entering this diagnostic.
fn mergeable_kinds(left: MergeKind, right: MergeKind) -> bool {
    use MergeKind::*;
    matches!(
        (left, right),
        (Interface, Interface | Class) | (Class, Interface) | (Enum, Enum) | (Namespace, Namespace)
    )
}

fn report_namespace_export_mismatches(
    items: &[(TopLevelItem<'_>, bool)],
    ctx: &mut CheckContext<'_, '_>,
) {
    let declarations = named_declarations(items);
    // Only declarations of one name can disagree, so pair them within a name instead of
    // across the whole scope: a file with many declarations would otherwise compare every
    // one with every later one. The pairs are sorted back into scope order so the reports
    // come out as they did when all pairs were visited in turn.
    let mut by_name: std::collections::HashMap<&str, Vec<usize>> = std::collections::HashMap::new();
    for (index, declaration) in declarations.iter().enumerate() {
        by_name
            .entry(declaration.name.as_str())
            .or_default()
            .push(index);
    }
    let mut pairs = Vec::new();
    for indexes in by_name.values().filter(|indexes| indexes.len() > 1) {
        for (at, &left) in indexes.iter().enumerate() {
            for &right in &indexes[at + 1..] {
                let (left_declaration, right_declaration) =
                    (&declarations[left], &declarations[right]);
                if left_declaration.exported != right_declaration.exported
                    && mergeable_kinds(left_declaration.kind, right_declaration.kind)
                {
                    pairs.push((left, right));
                }
            }
        }
    }
    pairs.sort_unstable();
    let mut reported = std::collections::HashSet::new();
    for (left, right) in pairs {
        let (left, right) = (&declarations[left], &declarations[right]);
        for span in [left.span, right.span] {
            if reported.insert((left.name.clone(), span.start)) {
                ctx.error(messages::namespace_export_mismatch(&left.name), span);
            }
        }
    }

    // Each namespace body is its own declaration scope. A nested namespace in two
    // separate parent blocks is checked independently here; its cross-block exports
    // are joined by report_cross_block_namespace_duplicates below.
    for (item, _) in items.iter().copied() {
        match item {
            TopLevelItem::Namespace(namespace) => {
                report_namespace_export_mismatches(&namespace_members(namespace), ctx);
            }
            TopLevelItem::ExternalModule(module) => {
                if let Some(body) = &module.body {
                    report_namespace_export_mismatches(&items_in(&body.body), ctx);
                }
            }
            _ => {}
        }
    }
}

fn report_enum_merge_initializers(
    items: &[(TopLevelItem<'_>, bool)],
    ctx: &mut CheckContext<'_, '_>,
) {
    let mut groups: BTreeMap<String, Vec<&TSEnumDeclaration<'_>>> = BTreeMap::new();
    for (item, _) in items.iter().copied() {
        if let TopLevelItem::Enum(decl) = item {
            groups
                .entry(decl.id.name.to_string())
                .or_default()
                .push(decl);
        }
    }
    for declarations in groups.values() {
        let mut seen_uninitialized_first_member = false;
        for declaration in declarations {
            let Some(first_member) = declaration.body.members.first() else {
                continue;
            };
            if first_member.initializer.is_none() {
                // Exactly one enum declaration in a merged enum may omit the
                // initializer on its first member. The first omission is allowed;
                // every later omission is TS2432.
                if seen_uninitialized_first_member {
                    let span = first_member.span();
                    if !ctx.diagnostics.iter().any(|diagnostic| {
                        diagnostic.code
                            == DiagnosticCode::EnumMultipleDeclarationsRequireInitializer
                            && diagnostic.start == span.start
                    }) {
                        ctx.error(
                            messages::enum_multiple_declarations_require_initializer(),
                            span,
                        );
                    }
                }
                seen_uninitialized_first_member = true;
            }
        }
    }

    // Exported declarations from separate blocks of one namespace share one enum
    // symbol. Recurse over their combined exported members so nested namespaces
    // are grouped as well; private declarations remain block-local.
    let mut namespace_groups: BTreeMap<String, Vec<&TSNamespaceDeclaration<'_>>> = BTreeMap::new();
    for (item, _) in items.iter().copied() {
        if let TopLevelItem::Namespace(namespace) = item {
            namespace_groups
                .entry(namespace.id.name.to_string())
                .or_default()
                .push(namespace);
        }
    }
    for blocks in namespace_groups.values() {
        if blocks.len() < 2 {
            continue;
        }
        let merged_exports: Vec<(TopLevelItem<'_>, bool)> = blocks
            .iter()
            .flat_map(|block| namespace_members(block))
            .filter(|(_, exported)| *exported)
            .collect();
        report_enum_merge_initializers(&merged_exports, ctx);
    }

    for (item, _) in items.iter().copied() {
        match item {
            TopLevelItem::Namespace(namespace) => {
                report_enum_merge_initializers(&namespace_members(namespace), ctx);
            }
            TopLevelItem::ExternalModule(module) => {
                if let Some(body) = &module.body {
                    report_enum_merge_initializers(&items_in(&body.body), ctx);
                }
            }
            _ => {}
        }
    }
}

fn report_namespace_merge_order(
    items: &[(TopLevelItem<'_>, bool)],
    ctx: &mut CheckContext<'_, '_>,
) {
    for (index, (item, _)) in items.iter().copied().enumerate() {
        if let TopLevelItem::Namespace(namespace) = item {
            let name = namespace.id.name.as_str();
            let is_after_class_or_function =
                items.iter().skip(index + 1).any(|(later, _)| match later {
                    TopLevelItem::Class(class) => {
                        class.id.as_ref().is_some_and(|id| id.name == name)
                    }
                    TopLevelItem::Function(function) => {
                        function.id.as_ref().is_some_and(|id| id.name == name)
                    }
                    _ => false,
                });
            if is_after_class_or_function
                && !ctx.diagnostics.iter().any(|diagnostic| {
                    diagnostic.code == DiagnosticCode::NamespaceMergeOrder
                        && diagnostic.start == namespace.id.span.start
                })
            {
                ctx.error(messages::namespace_merge_order(), namespace.id.span);
            }
            report_namespace_merge_order(&namespace_members(namespace), ctx);
        } else if let TopLevelItem::ExternalModule(module) = item {
            if let Some(body) = &module.body {
                report_namespace_merge_order(&items_in(&body.body), ctx);
            }
        }
    }

    // Declarations exported from separate blocks of one namespace share a merged
    // namespace body. Compare their source order too: `namespace A { export
    // namespace B {} }` followed by `namespace A { export class B {} }` is TS2434.
    let mut namespace_groups: BTreeMap<String, Vec<&TSNamespaceDeclaration<'_>>> = BTreeMap::new();
    for (item, _) in items.iter().copied() {
        if let TopLevelItem::Namespace(namespace) = item {
            namespace_groups
                .entry(namespace.id.name.to_string())
                .or_default()
                .push(namespace);
        }
    }
    for blocks in namespace_groups.values() {
        if blocks.len() < 2 {
            continue;
        }
        let merged_exports: Vec<(TopLevelItem<'_>, bool)> = blocks
            .iter()
            .flat_map(|block| namespace_members(block))
            .filter(|(_, exported)| *exported)
            .collect();
        report_namespace_merge_order(&merged_exports, ctx);
    }
}

// Revisit only namespace exports across blocks of the same namespace. The normal
// walker has already handled each block's private scope and same-block duplicates.
// This pass catches exported `let`/`const` and enum-member collisions that otherwise
// disappear because the AST gives each namespace block a separate lexical scope.
fn report_cross_block_namespace_duplicates(
    items: &[(TopLevelItem<'_>, bool)],
    out: &mut Vec<Found>,
) {
    let mut groups: BTreeMap<String, Vec<&TSNamespaceDeclaration<'_>>> = BTreeMap::new();
    for (item, _) in items.iter().copied() {
        if let TopLevelItem::Namespace(namespace) = item {
            groups
                .entry(namespace.id.name.to_string())
                .or_default()
                .push(namespace);
        }
    }

    for blocks in groups.into_values() {
        let members: Vec<(TopLevelItem<'_>, bool)> = blocks
            .iter()
            .flat_map(|block| namespace_members(block))
            .collect();
        let mut scope = Scope {
            names: HashMap::new(),
            function_level: true,
        };
        let mut nested_namespaces = Vec::new();
        for (item, exported) in members.iter().copied() {
            if !exported {
                continue;
            }
            match item {
                TopLevelItem::Alias(decl) => {
                    Walker::merge(
                        out,
                        scope.names.entry(decl.id.name.to_string()).or_default(),
                        &decl.id.name,
                        TYPE_ALIAS,
                        decl.id.span,
                    );
                }
                TopLevelItem::Interface(decl) => {
                    Walker::merge(
                        out,
                        scope.names.entry(decl.id.name.to_string()).or_default(),
                        &decl.id.name,
                        INTERFACE,
                        decl.id.span,
                    );
                }
                TopLevelItem::Class(class) => {
                    if let Some(id) = &class.id {
                        Walker::merge(
                            out,
                            scope.names.entry(id.name.to_string()).or_default(),
                            &id.name,
                            CLASS,
                            id.span,
                        );
                    }
                }
                TopLevelItem::Enum(decl) => {
                    let flags = if decl.r#const {
                        CONST_ENUM
                    } else {
                        REGULAR_ENUM
                    };
                    let symbol = scope.names.entry(decl.id.name.to_string()).or_default();
                    Walker::merge(out, symbol, &decl.id.name, flags, decl.id.span);
                    if symbol.flags & ENUM != 0 {
                        for member in &decl.body.members {
                            let (member_name, span) = match &member.id {
                                TSEnumMemberName::Identifier(id) => (id.name.to_string(), id.span),
                                TSEnumMemberName::String(literal) => {
                                    (literal.value.to_string(), literal.span)
                                }
                                _ => continue,
                            };
                            Walker::merge(
                                out,
                                symbol.members.entry(member_name.clone()).or_default(),
                                &member_name,
                                ENUM_MEMBER,
                                span,
                            );
                        }
                    }
                }
                TopLevelItem::Variable(decl) => {
                    let flags = if decl.kind == VariableDeclarationKind::Var {
                        FUNCTION_SCOPED
                    } else {
                        BLOCK_SCOPED
                    };
                    for declarator in &decl.declarations {
                        let mut bindings = Vec::new();
                        binding_names(&declarator.id, &mut bindings);
                        for (name, span) in bindings {
                            Walker::merge(
                                out,
                                scope.names.entry(name.clone()).or_default(),
                                &name,
                                flags,
                                span,
                            );
                        }
                    }
                }
                TopLevelItem::Function(function) => {
                    if let Some(id) = &function.id {
                        Walker::merge(
                            out,
                            scope.names.entry(id.name.to_string()).or_default(),
                            &id.name,
                            FUNCTION,
                            id.span,
                        );
                    }
                }
                TopLevelItem::Namespace(namespace) => {
                    nested_namespaces.push((TopLevelItem::Namespace(namespace), true));
                    let flags = if namespace_instantiated(namespace) {
                        VALUE_MODULE
                    } else {
                        NAMESPACE_MODULE
                    };
                    Walker::merge(
                        out,
                        scope
                            .names
                            .entry(namespace.id.name.to_string())
                            .or_default(),
                        &namespace.id.name,
                        flags,
                        namespace.id.span,
                    );
                }
                TopLevelItem::ExternalModule(_) => {}
            }
        }
        report_cross_block_namespace_duplicates(&nested_namespaces, out);
    }

    for (item, _) in items.iter().copied() {
        if let TopLevelItem::ExternalModule(module) = item
            && let Some(body) = &module.body
        {
            report_cross_block_namespace_duplicates(&items_in(&body.body), out);
        }
    }
}

impl Walker {
    fn push(&mut self, function_level: bool) {
        self.scopes.push(Scope {
            names: HashMap::new(),
            function_level,
        });
    }

    fn pop(&mut self) {
        self.scopes.pop();
    }

    fn declare_in(&mut self, scope: usize, name: &str, flags: u32, span: Span) -> usize {
        let table = &mut self.scopes[scope].names;
        let symbol = table.entry(name.to_string()).or_default();
        Self::merge(&mut self.found, symbol, name, flags, span);
        scope
    }

    fn merge(found: &mut Vec<Found>, symbol: &mut Symbol, name: &str, flags: u32, span: Span) {
        if symbol.flags & excludes(flags) != 0 {
            let enum_merge = (symbol.flags | flags) & ENUM != 0;
            let block_scoped = symbol.flags & BLOCK_SCOPED != 0;
            for earlier in &symbol.declarations {
                found.push(Found {
                    name: name.to_string(),
                    span: *earlier,
                    block_scoped,
                    enum_merge,
                });
            }
            found.push(Found {
                name: name.to_string(),
                span,
                block_scoped,
                enum_merge,
            });
            // tsc keeps the original symbol in the table and puts the conflicting
            // declaration on a fresh one nobody sees again.
            return;
        }
        symbol.flags |= flags;
        symbol.declarations.push(span);
    }

    fn declare(&mut self, name: &str, flags: u32, span: Span) -> usize {
        let scope = self.scopes.len() - 1;
        self.declare_in(scope, name, flags, span)
    }

    fn declare_hoisted(&mut self, name: &str, span: Span) {
        let scope = self
            .scopes
            .iter()
            .rposition(|scope| scope.function_level)
            .unwrap_or(0);
        self.declare_in(scope, name, FUNCTION_SCOPED, span);
    }

    fn declare_pattern(&mut self, pattern: &BindingPattern<'_>, flags: u32, hoisted: bool) {
        match pattern {
            BindingPattern::BindingIdentifier(id) => {
                if hoisted {
                    self.declare_hoisted(&id.name, id.span);
                } else {
                    self.declare(&id.name, flags, id.span);
                }
            }
            BindingPattern::ObjectPattern(object) => {
                for property in &object.properties {
                    self.declare_pattern(&property.value, flags, hoisted);
                }
                if let Some(rest) = &object.rest {
                    self.declare_pattern(&rest.argument, flags, hoisted);
                }
            }
            BindingPattern::ArrayPattern(array) => {
                for element in array.elements.iter().flatten() {
                    self.declare_pattern(element, flags, hoisted);
                }
                if let Some(rest) = &array.rest {
                    self.declare_pattern(&rest.argument, flags, hoisted);
                }
            }
            BindingPattern::AssignmentPattern(assign) => {
                self.declare_pattern(&assign.left, flags, hoisted);
            }
        }
    }

    fn declare_params(&mut self, params: &oxc_ast::ast::FormalParameters<'_>) {
        for param in &params.items {
            self.declare_pattern(&param.pattern, FUNCTION_SCOPED, false);
        }
        if let Some(rest) = &params.rest {
            self.declare_pattern(&rest.rest.argument, FUNCTION_SCOPED, false);
        }
    }
}

// A namespace with any value in it is instantiated and is a value; one with only types
// is not and merges with everything.
fn instantiated(statements: &[Statement<'_>]) -> bool {
    statements.iter().any(statement_is_value)
}

fn statement_is_value(statement: &Statement<'_>) -> bool {
    match statement {
        Statement::TSInterfaceDeclaration(_)
        | Statement::TSTypeAliasDeclaration(_)
        | Statement::TSImportEqualsDeclaration(_) => false,
        Statement::TSEnumDeclaration(decl) => !decl.r#const,
        Statement::TSNamespaceDeclaration(decl) => namespace_instantiated(decl),
        Statement::ExportDeclaration(export) => declaration_is_value(&export.declaration),
        Statement::ExportNamedDeclaration(_) => false,
        Statement::ExportDefaultDeclaration(_) => true,
        Statement::ExportAllDeclaration(_) | Statement::ImportDeclaration(_) => false,
        _ => true,
    }
}

fn declaration_is_value(declaration: &oxc_ast::ast::Declaration<'_>) -> bool {
    use oxc_ast::ast::Declaration;
    match declaration {
        Declaration::TSInterfaceDeclaration(_)
        | Declaration::TSTypeAliasDeclaration(_)
        | Declaration::TSImportEqualsDeclaration(_) => false,
        Declaration::TSEnumDeclaration(decl) => !decl.r#const,
        Declaration::TSNamespaceDeclaration(decl) => namespace_instantiated(decl),
        _ => true,
    }
}

fn namespace_instantiated(decl: &TSNamespaceDeclaration<'_>) -> bool {
    match &decl.body {
        TSNamespaceDeclarationBody::TSModuleBlock(block) => instantiated(&block.body),
        TSNamespaceDeclarationBody::TSNamespaceDeclaration(inner) => namespace_instantiated(inner),
    }
}

impl<'a> Visit<'a> for Walker {
    fn visit_program(&mut self, program: &Program<'a>) {
        self.push(true);
        walk::walk_program(self, program);
        self.pop();
    }

    fn visit_function(&mut self, func: &Function<'a>, flags: ScopeFlags) {
        let declaration = matches!(
            func.r#type,
            FunctionType::FunctionDeclaration | FunctionType::TSDeclareFunction
        );
        if declaration && let Some(id) = &func.id {
            self.declare(&id.name, FUNCTION, id.span);
        }
        self.push(true);
        self.declare_params(&func.params);
        walk::walk_function(self, func, flags);
        self.pop();
    }

    fn visit_arrow_function_expression(&mut self, arrow: &ArrowFunctionExpression<'a>) {
        self.push(true);
        self.declare_params(&arrow.params);
        walk::walk_arrow_function_expression(self, arrow);
        self.pop();
    }

    fn visit_static_block(&mut self, block: &StaticBlock<'a>) {
        self.push(true);
        walk::walk_static_block(self, block);
        self.pop();
    }

    fn visit_block_statement(&mut self, block: &BlockStatement<'a>) {
        self.push(false);
        walk::walk_block_statement(self, block);
        self.pop();
    }

    fn visit_for_statement(&mut self, statement: &ForStatement<'a>) {
        self.push(false);
        walk::walk_for_statement(self, statement);
        self.pop();
    }

    fn visit_for_in_statement(&mut self, statement: &ForInStatement<'a>) {
        self.push(false);
        walk::walk_for_in_statement(self, statement);
        self.pop();
    }

    fn visit_for_of_statement(&mut self, statement: &ForOfStatement<'a>) {
        self.push(false);
        walk::walk_for_of_statement(self, statement);
        self.pop();
    }

    fn visit_switch_statement(&mut self, statement: &SwitchStatement<'a>) {
        self.visit_expression(&statement.discriminant);
        self.push(false);
        for case in &statement.cases {
            self.visit_switch_case(case);
        }
        self.pop();
    }

    fn visit_variable_declaration(&mut self, declaration: &VariableDeclaration<'a>) {
        let hoisted = declaration.kind == VariableDeclarationKind::Var;
        let flags = if hoisted {
            FUNCTION_SCOPED
        } else {
            BLOCK_SCOPED
        };
        for declarator in &declaration.declarations {
            self.declare_pattern(&declarator.id, flags, hoisted);
        }
        walk::walk_variable_declaration(self, declaration);
    }

    fn visit_class(&mut self, class: &Class<'a>) {
        if class.r#type == ClassType::ClassDeclaration
            && let Some(id) = &class.id
        {
            self.declare(&id.name, CLASS, id.span);
        }
        walk::walk_class(self, class);
    }

    fn visit_ts_interface_declaration(&mut self, decl: &TSInterfaceDeclaration<'a>) {
        self.declare(&decl.id.name, INTERFACE, decl.id.span);
        walk::walk_ts_interface_declaration(self, decl);
    }

    fn visit_ts_type_alias_declaration(&mut self, decl: &TSTypeAliasDeclaration<'a>) {
        self.declare(&decl.id.name, TYPE_ALIAS, decl.id.span);
        walk::walk_ts_type_alias_declaration(self, decl);
    }

    fn visit_ts_enum_declaration(&mut self, decl: &TSEnumDeclaration<'a>) {
        let flags = if decl.r#const {
            CONST_ENUM
        } else {
            REGULAR_ENUM
        };
        let scope = self.scopes.len() - 1;
        self.declare_in(scope, &decl.id.name, flags, decl.id.span);
        // The members of every declaration of the enum share one table.
        let Self { scopes, found } = self;
        if let Some(symbol) = scopes[scope].names.get_mut(decl.id.name.as_str())
            && symbol.flags & ENUM != 0
        {
            for member in &decl.body.members {
                let (name, span) = match &member.id {
                    TSEnumMemberName::Identifier(id) => (id.name.to_string(), id.span),
                    TSEnumMemberName::String(lit) => (lit.value.to_string(), lit.span),
                    _ => continue,
                };
                let entry = symbol.members.entry(name.clone()).or_default();
                Self::merge(found, entry, &name, ENUM_MEMBER, span);
            }
        }
        walk::walk_ts_enum_declaration(self, decl);
    }

    fn visit_ts_namespace_declaration(&mut self, decl: &TSNamespaceDeclaration<'a>) {
        let flags = if namespace_instantiated(decl) {
            VALUE_MODULE
        } else {
            NAMESPACE_MODULE
        };
        self.declare(&decl.id.name, flags, decl.id.span);
        self.push(true);
        walk::walk_ts_namespace_declaration(self, decl);
        self.pop();
    }

    fn visit_ts_module_block(&mut self, block: &TSModuleBlock<'a>) {
        walk::walk_ts_module_block(self, block);
    }

    fn visit_ts_external_module_declaration(&mut self, decl: &TSExternalModuleDeclaration<'a>) {
        self.push(true);
        walk::walk_ts_external_module_declaration(self, decl);
        self.pop();
    }

    fn visit_import_declaration(&mut self, decl: &ImportDeclaration<'a>) {
        for specifier in decl.specifiers.iter().flatten() {
            let local = match specifier {
                ImportDeclarationSpecifier::ImportSpecifier(s) => &s.local,
                ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => &s.local,
                ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => &s.local,
            };
            self.declare(&local.name, ALIAS, local.span);
        }
    }

    fn visit_ts_import_equals_declaration(&mut self, decl: &TSImportEqualsDeclaration<'a>) {
        self.declare(&decl.id.name, ALIAS, decl.id.span);
        walk::walk_ts_import_equals_declaration(self, decl);
    }

    fn visit_ts_export_assignment(&mut self, assignment: &TSExportAssignment<'a>) {
        self.declare("export=", ALIAS, assignment.expression.span());
        walk::walk_ts_export_assignment(self, assignment);
    }
}
