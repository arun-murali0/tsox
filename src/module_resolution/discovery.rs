use oxc_allocator::Allocator;
use oxc_ast_visit::{Visit, walk};
use oxc_parser::Parser;
use oxc_span::SourceType;
use std::collections::HashMap;

// One import or re-export statement's request for another module, as the parser
// recorded it. `is_import` is false for `export ... from`, which depends on the other
// module just as much but binds nothing in this one.
/// Why a source file depends on another module. This is deliberately separate from
/// `is_type`: type-only is a property of an import, while dynamic/static/re-export
/// describe how the edge participates in graph consumers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ModuleRequestKind {
    StaticImport,
    TypeImport,
    SideEffectImport,
    ReExport,
    DynamicImport,
    ImportEquals,
    ModuleAugmentation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleRequest {
    pub specifier: String,
    pub is_type: bool,
    /// Backwards-compatible summary: true when this statement imports/binds locally.
    /// New consumers should use `kind` instead.
    pub is_import: bool,
    pub kind: ModuleRequestKind,
    pub start: u32,
}

// What a file asks for, and whether the parse behind it can be trusted. A recoverable
// syntax error leaves the requests intact, since the parser still recorded every
// statement it got through; only an unrecoverable one empties them. In both cases
// `parse_failed` is set, so a file that lost edges is never mistaken for one that has
// none.
#[derive(Debug, Default)]
pub struct ModuleScan {
    pub requests: Vec<ModuleRequest>,
    pub parse_failed: bool,
    /// The file is a script (it has no `import` or `export`) that declares ambient modules
    /// with `declare module "x" { ... }`. Read off the parse the scan already made, so the
    /// checker does not parse the file again to find out.
    pub declares_ambient_modules: bool,
}

pub fn scan_module_requests(source: &str, file_name: &str) -> ModuleScan {
    // The same refusal the checker makes, before the parser can recurse into an overflow.
    if crate::source_depth::max_bracket_depth(source) > crate::stack_guard::MAX_BRACKET_DEPTH {
        return ModuleScan {
            requests: Vec::new(),
            parse_failed: true,
            declares_ambient_modules: false,
        };
    }
    let allocator = Allocator::default();
    let source_type = SourceType::from_path(file_name)
        .unwrap_or_else(|_| SourceType::default().with_typescript(true));
    let parsed = Parser::new(&allocator, source, source_type).parse();
    let parse_failed = parsed.diagnostics.has_errors();

    if parsed.fatal_error {
        return ModuleScan {
            requests: Vec::new(),
            parse_failed,
            declares_ambient_modules: false,
        };
    }

    // The module record is the parser's authoritative list of static requests, but it
    // intentionally normalizes syntax. Recover the syntax-level edge kind from the AST
    // using the source-literal start offset; don't infer it from source text.
    let mut kinds_by_start = HashMap::new();
    for statement in &parsed.program.body {
        match statement {
            oxc_ast::ast::Statement::ImportDeclaration(decl) => {
                let kind = if decl.import_kind == oxc_ast::ast::ImportOrExportKind::Type {
                    ModuleRequestKind::TypeImport
                } else if decl
                    .specifiers
                    .as_ref()
                    .is_none_or(|specifiers| specifiers.is_empty())
                {
                    ModuleRequestKind::SideEffectImport
                } else {
                    ModuleRequestKind::StaticImport
                };
                kinds_by_start.insert(decl.source.span.start, kind);
                kinds_by_start.insert(decl.span.start, kind);
            }
            oxc_ast::ast::Statement::ExportFromDeclaration(decl) => {
                kinds_by_start.insert(decl.source.span.start, ModuleRequestKind::ReExport);
                kinds_by_start.insert(decl.span.start, ModuleRequestKind::ReExport);
            }
            oxc_ast::ast::Statement::ExportAllDeclaration(decl) => {
                kinds_by_start.insert(decl.source.span.start, ModuleRequestKind::ReExport);
                kinds_by_start.insert(decl.span.start, ModuleRequestKind::ReExport);
            }
            _ => {}
        }
    }

    let mut requests = Vec::new();
    for (specifier, entries) in parsed.module_record.requested_modules.iter() {
        for entry in entries.iter() {
            let mapped = kinds_by_start.get(&entry.span.start).copied();
            let kind = match mapped {
                // A type-only re-export is still a re-export edge; `is_type` remains
                // available separately for resolution policy.
                Some(ModuleRequestKind::ReExport) => ModuleRequestKind::ReExport,
                Some(ModuleRequestKind::SideEffectImport) => ModuleRequestKind::SideEffectImport,
                Some(ModuleRequestKind::TypeImport) => ModuleRequestKind::TypeImport,
                Some(ModuleRequestKind::StaticImport) if entry.is_type => {
                    ModuleRequestKind::TypeImport
                }
                Some(kind) => kind,
                None if entry.is_type => ModuleRequestKind::TypeImport,
                None if entry.is_import => ModuleRequestKind::StaticImport,
                None => ModuleRequestKind::ReExport,
            };
            requests.push(ModuleRequest {
                specifier: specifier.as_str().to_owned(),
                is_type: entry.is_type,
                is_import: kind != ModuleRequestKind::ReExport,
                kind,
                start: entry.span.start,
            });
        }
    }
    // The parser's module record covers `import` and `export ... from` only. Two more
    // forms ask for a module and are read off the syntax tree: `import x = require("m")`
    // at the top level, and `import("m")` with a plain string for its argument.
    let is_external_module = parsed.program.body.iter().any(|statement| {
        matches!(
            statement,
            oxc_ast::ast::Statement::ImportDeclaration(_)
                | oxc_ast::ast::Statement::ExportDeclaration(_)
                | oxc_ast::ast::Statement::ExportNamedDeclaration(_)
                | oxc_ast::ast::Statement::ExportDefaultDeclaration(_)
                | oxc_ast::ast::Statement::ExportAllDeclaration(_)
                | oxc_ast::ast::Statement::ExportFromDeclaration(_)
                | oxc_ast::ast::Statement::TSExportAssignment(_)
                | oxc_ast::ast::Statement::TSNamespaceExportDeclaration(_)
                | oxc_ast::ast::Statement::TSImportEqualsDeclaration(_)
        )
    });
    for statement in &parsed.program.body {
        if let oxc_ast::ast::Statement::TSImportEqualsDeclaration(decl) = statement {
            if let oxc_ast::ast::TSModuleReference::ExternalModuleReference(reference) =
                &decl.module_reference
            {
                requests.push(ModuleRequest {
                    specifier: reference.expression.value.as_str().to_owned(),
                    is_type: decl.import_kind == oxc_ast::ast::ImportOrExportKind::Type,
                    is_import: true,
                    kind: ModuleRequestKind::ImportEquals,
                    start: decl.span.start,
                });
            }
        }
        if is_external_module
            && let oxc_ast::ast::Statement::TSExternalModuleDeclaration(module) = statement
            && let Some(body) = &module.body
        {
            // An ambient external-module declaration in a file that is itself a module
            // augments the resolved module. Keep it as a typed graph edge so that the
            // target is discovered and checked before the augmentation is consumed.
            // Script-style ambient module declarations are providers, not augmentations,
            // and intentionally do not create this edge.
            let _ = body;
            requests.push(ModuleRequest {
                specifier: module.id.value.as_str().to_owned(),
                is_type: false,
                is_import: false,
                kind: ModuleRequestKind::ModuleAugmentation,
                start: module.span.start,
            });
        }
    }
    // Read dynamic imports from the AST rather than slicing their source text. Oxc's
    // string-literal value is already escape-decoded, so `import('./\x6cazy')` resolves
    // to the same file as `import('./lazy')`. Non-literal arguments remain runtime-only
    // and do not create a speculative filesystem dependency.
    struct DynamicImportWalker {
        requests: Vec<ModuleRequest>,
    }
    impl<'a> Visit<'a> for DynamicImportWalker {
        fn visit_import_expression(&mut self, expression: &oxc_ast::ast::ImportExpression<'a>) {
            if let oxc_ast::ast::Expression::StringLiteral(specifier) = &expression.source {
                self.requests.push(ModuleRequest {
                    specifier: specifier.value.as_str().to_owned(),
                    is_type: false,
                    is_import: true,
                    kind: ModuleRequestKind::DynamicImport,
                    start: expression.span.start,
                });
            }
            walk::walk_import_expression(self, expression);
        }
    }
    let mut dynamic_imports = DynamicImportWalker {
        requests: Vec::new(),
    };
    dynamic_imports.visit_program(&parsed.program);
    requests.extend(dynamic_imports.requests);
    // The parser keeps requests in a hash map keyed by specifier, whose iteration order
    // changes from run to run. Source order makes the graph reproducible.
    requests.sort_by_key(|request| request.start);

    let declares_ambient_modules = !is_external_module
        && parsed.program.body.iter().any(|statement| {
            matches!(
                statement,
                oxc_ast::ast::Statement::TSExternalModuleDeclaration(_)
            )
        });

    ModuleScan {
        requests,
        parse_failed,
        declares_ambient_modules,
    }
}

#[cfg(test)]
mod tests {
    use super::scan_module_requests;

    #[test]
    fn requests_come_back_in_source_order() {
        let scan = scan_module_requests(
            "import { b } from './b';\nimport type { C } from './c';\nexport * from './a';",
            "main.ts",
        );
        let specifiers: Vec<_> = scan
            .requests
            .iter()
            .map(|request| request.specifier.as_str())
            .collect();
        assert_eq!(specifiers, ["./b", "./c", "./a"]);
        assert!(!scan.parse_failed);
    }

    #[test]
    fn type_only_and_re_export_requests_are_told_apart() {
        let scan = scan_module_requests(
            "import type { T } from './t';\nexport { x } from './x';\nimport { v } from './v';",
            "main.ts",
        );
        let flags: Vec<_> = scan
            .requests
            .iter()
            .map(|request| (request.is_type, request.is_import, request.kind))
            .collect();
        assert_eq!(
            flags,
            [
                (true, true, super::ModuleRequestKind::TypeImport),
                (false, false, super::ModuleRequestKind::ReExport),
                (false, true, super::ModuleRequestKind::StaticImport),
            ]
        );
    }

    #[test]
    fn edge_kinds_distinguish_side_effect_dynamic_and_import_equals_requests() {
        let scan = scan_module_requests(
            "import './polyfill';\nimport('./lazy');\nimport Legacy = require('./legacy');",
            "main.ts",
        );
        let got: Vec<_> = scan
            .requests
            .iter()
            .map(|r| (r.specifier.as_str(), r.kind))
            .collect();
        assert_eq!(
            got,
            [
                ("./polyfill", super::ModuleRequestKind::SideEffectImport),
                ("./lazy", super::ModuleRequestKind::DynamicImport),
                ("./legacy", super::ModuleRequestKind::ImportEquals),
            ]
        );
    }

    #[test]
    fn non_literal_dynamic_import_is_not_fabricated_into_a_static_edge() {
        let scan = scan_module_requests("const name = getName(); import(name);", "main.ts");
        assert!(scan.requests.is_empty());
    }

    #[test]
    fn escaped_dynamic_import_uses_the_decoded_module_specifier() {
        let scan = scan_module_requests("import('./\\x6cazy');", "main.ts");
        assert_eq!(scan.requests.len(), 1);
        assert_eq!(scan.requests[0].specifier, "./lazy");
        assert_eq!(
            scan.requests[0].kind,
            super::ModuleRequestKind::DynamicImport
        );
    }

    #[test]
    fn a_syntax_error_is_reported_and_not_mistaken_for_no_imports() {
        let scan = scan_module_requests("import { a } from './a'; const = ;", "broken.ts");
        assert!(scan.parse_failed);
    }
}
