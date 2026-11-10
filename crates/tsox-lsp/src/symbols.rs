//! The outline of a file: its declarations as a tree, for `textDocument/documentSymbol`.
//!
//! This reads the syntax tree and nothing else, so it needs no check, no project and no
//! type information, and it works on a file that has syntax errors: the parser recovers,
//! and what it could read is outlined. A file the parser could read nothing of has an empty
//! outline.

use oxc_ast::ast::{
    BindingPattern, Class, ClassElement, Declaration, ExportDefaultDeclarationKind, Expression,
    Function, MethodDefinitionKind, ModuleDeclaration, PropertyKey, Statement, TSEnumMemberName,
    TSNamespaceDeclaration, TSNamespaceDeclarationBody, TSSignature, VariableDeclarationKind,
};
use oxc_span::{GetSpan, Span};
use tower_lsp_server::ls_types::{DocumentSymbol, Location, SymbolInformation, SymbolKind, Uri};

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::navigate::Host;
use crate::text::LineMap;

/// One declaration. Offsets are bytes into the file's text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub detail: Option<String>,
    pub kind: SymbolKind,
    /// The whole declaration, with its `export` and body.
    pub range: (u32, u32),
    /// The name, which is what the editor highlights and jumps to.
    pub selection: (u32, u32),
    pub children: Vec<Symbol>,
}

/// The outline of `text`, read as the dialect `file_name`'s extension names (TypeScript when
/// it names none). `None` for a file nested too deeply to be parsed safely.
pub fn outline(text: &str, file_name: &str) -> Option<Vec<Symbol>> {
    crate::analysis::with_program(text, file_name, |program| {
        Reader { text }.statements(&program.body)
    })
}

struct Reader<'t> {
    text: &'t str,
}

type Range32 = (u32, u32);

fn range(span: Span) -> Range32 {
    (span.start, span.end)
}

impl Reader<'_> {
    fn slice(&self, span: Span) -> &str {
        self.text
            .get(span.start as usize..span.end as usize)
            .unwrap_or("")
    }

    fn statements(&self, statements: &[Statement<'_>]) -> Vec<Symbol> {
        let mut out = Vec::new();
        for statement in statements {
            self.statement(statement, &mut out);
        }
        out
    }

    fn statement(&self, statement: &Statement<'_>, out: &mut Vec<Symbol>) {
        if let Some(declaration) = statement.as_declaration() {
            self.declaration(declaration, range(declaration.span()), out);
        } else if let Some(module) = statement.as_module_declaration() {
            match module {
                ModuleDeclaration::ExportDeclaration(export) => {
                    self.declaration(&export.declaration, range(export.span), out);
                }
                ModuleDeclaration::ExportDefaultDeclaration(export) => {
                    self.export_default(&export.declaration, range(export.span), out);
                }
                _ => {}
            }
        }
    }

    fn export_default(
        &self,
        declaration: &ExportDefaultDeclarationKind<'_>,
        whole: Range32,
        out: &mut Vec<Symbol>,
    ) {
        match declaration {
            ExportDefaultDeclarationKind::FunctionDeclaration(function) => {
                out.push(self.function(function, whole, "default"));
            }
            ExportDefaultDeclarationKind::ClassDeclaration(class) => {
                out.push(self.class(class, whole, "default"));
            }
            ExportDefaultDeclarationKind::TSInterfaceDeclaration(interface) => {
                out.push(self.interface(interface, whole));
            }
            // `export default <expression>` names nothing.
            _ => {}
        }
    }

    fn declaration(&self, declaration: &Declaration<'_>, whole: Range32, out: &mut Vec<Symbol>) {
        match declaration {
            Declaration::FunctionDeclaration(function) => {
                // `declare function f(): void;` overloads and ambient functions are outlined
                // too; one with no name (not valid outside a default export) is skipped.
                if function.id.is_some() {
                    out.push(self.function(function, whole, "function"));
                }
            }
            Declaration::ClassDeclaration(class) => {
                if class.id.is_some() {
                    out.push(self.class(class, whole, "class"));
                }
            }
            Declaration::VariableDeclaration(variables) => {
                let single = variables.declarations.len() == 1;
                let constant = variables.kind == VariableDeclarationKind::Const
                    || variables.kind == VariableDeclarationKind::Using
                    || variables.kind == VariableDeclarationKind::AwaitUsing;
                for declarator in &variables.declarations {
                    let whole = if single {
                        whole
                    } else {
                        range(declarator.span)
                    };
                    let callable = matches!(
                        declarator.init,
                        Some(
                            Expression::ArrowFunctionExpression(_)
                                | Expression::FunctionExpression(_)
                        )
                    );
                    let kind = if callable {
                        SymbolKind::FUNCTION
                    } else if constant {
                        SymbolKind::CONSTANT
                    } else {
                        SymbolKind::VARIABLE
                    };
                    let detail = declarator
                        .type_annotation
                        .as_ref()
                        .map(|annotation| self.slice(annotation.type_annotation.span()).to_owned());
                    self.bindings(&declarator.id, kind, whole, detail.as_deref(), out);
                }
            }
            Declaration::TSTypeAliasDeclaration(alias) => out.push(Symbol {
                name: alias.id.name.to_string(),
                detail: None,
                kind: SymbolKind::TYPE_PARAMETER,
                range: whole,
                selection: range(alias.id.span),
                children: Vec::new(),
            }),
            Declaration::TSInterfaceDeclaration(interface) => {
                out.push(self.interface(interface, whole))
            }
            Declaration::TSEnumDeclaration(declaration) => {
                let members = declaration
                    .body
                    .members
                    .iter()
                    .map(|member| {
                        let (name, selection) = match &member.id {
                            TSEnumMemberName::Identifier(name) => {
                                (name.name.to_string(), range(name.span))
                            }
                            other => (self.slice(other.span()).to_owned(), range(other.span())),
                        };
                        Symbol {
                            name,
                            detail: None,
                            kind: SymbolKind::ENUM_MEMBER,
                            range: range(member.span),
                            selection,
                            children: Vec::new(),
                        }
                    })
                    .collect();
                out.push(Symbol {
                    name: declaration.id.name.to_string(),
                    detail: declaration.r#const.then(|| "const".to_owned()),
                    kind: SymbolKind::ENUM,
                    range: whole,
                    selection: range(declaration.id.span),
                    children: members,
                });
            }
            Declaration::TSNamespaceDeclaration(namespace) => {
                out.push(self.namespace(namespace, whole));
            }
            Declaration::TSExternalModuleDeclaration(module) => out.push(Symbol {
                name: self.slice(module.id.span).to_owned(),
                detail: None,
                kind: SymbolKind::MODULE,
                range: whole,
                selection: range(module.id.span),
                children: module
                    .body
                    .as_ref()
                    .map(|block| self.statements(&block.body))
                    .unwrap_or_default(),
            }),
            Declaration::TSGlobalDeclaration(global) => out.push(Symbol {
                name: "global".to_owned(),
                detail: None,
                kind: SymbolKind::NAMESPACE,
                range: whole,
                selection: range(global.global_span),
                children: self.statements(&global.body.body),
            }),
            Declaration::TSImportEqualsDeclaration(_) => {}
        }
    }

    /// `namespace A.B.C {}` is a namespace inside a namespace in the tree; it is outlined as
    /// the one name it is written as, holding what the innermost block holds.
    fn namespace(&self, namespace: &TSNamespaceDeclaration<'_>, whole: Range32) -> Symbol {
        let mut name = namespace.id.name.to_string();
        let mut body = &namespace.body;
        let children = loop {
            match body {
                TSNamespaceDeclarationBody::TSNamespaceDeclaration(inner) => {
                    name.push('.');
                    name.push_str(inner.id.name.as_str());
                    body = &inner.body;
                }
                TSNamespaceDeclarationBody::TSModuleBlock(block) => {
                    break self.statements(&block.body);
                }
            }
        };
        Symbol {
            name,
            detail: None,
            kind: SymbolKind::NAMESPACE,
            range: whole,
            selection: range(namespace.id.span),
            children,
        }
    }

    /// The names a binding pattern introduces, each as a symbol of its own.
    fn bindings(
        &self,
        pattern: &BindingPattern<'_>,
        kind: SymbolKind,
        whole: Range32,
        detail: Option<&str>,
        out: &mut Vec<Symbol>,
    ) {
        match pattern {
            BindingPattern::BindingIdentifier(id) => out.push(Symbol {
                name: id.name.to_string(),
                detail: detail.map(str::to_owned),
                kind,
                range: whole,
                selection: range(id.span),
                children: Vec::new(),
            }),
            BindingPattern::ObjectPattern(object) => {
                for property in &object.properties {
                    self.bindings(&property.value, kind, whole, None, out);
                }
                if let Some(rest) = &object.rest {
                    self.bindings(&rest.argument, kind, whole, None, out);
                }
            }
            BindingPattern::ArrayPattern(array) => {
                for element in array.elements.iter().flatten() {
                    self.bindings(element, kind, whole, None, out);
                }
                if let Some(rest) = &array.rest {
                    self.bindings(&rest.argument, kind, whole, None, out);
                }
            }
            BindingPattern::AssignmentPattern(assignment) => {
                self.bindings(&assignment.left, kind, whole, detail, out);
            }
        }
    }

    fn function(&self, function: &Function<'_>, whole: Range32, fallback: &str) -> Symbol {
        let (name, selection) = match &function.id {
            Some(id) => (id.name.to_string(), range(id.span)),
            None => (fallback.to_owned(), (whole.0, whole.0)),
        };
        // Functions and classes declared inside this one belong under it.
        let children = function
            .body
            .as_ref()
            .map(|body| self.nested(&body.statements))
            .unwrap_or_default();
        Symbol {
            name,
            detail: None,
            kind: SymbolKind::FUNCTION,
            range: whole,
            selection,
            children,
        }
    }

    /// Only the functions and classes among a body's statements: local variables would
    /// bury the outline.
    fn nested(&self, statements: &[Statement<'_>]) -> Vec<Symbol> {
        self.statements(statements)
            .into_iter()
            .filter(|symbol| {
                symbol.kind == SymbolKind::FUNCTION || symbol.kind == SymbolKind::CLASS
            })
            .collect()
    }

    fn class(&self, class: &Class<'_>, whole: Range32, fallback: &str) -> Symbol {
        let (name, selection) = match &class.id {
            Some(id) => (id.name.to_string(), range(id.span)),
            None => (fallback.to_owned(), (whole.0, whole.0)),
        };
        let mut children = Vec::new();
        for element in &class.body.body {
            match element {
                ClassElement::MethodDefinition(method) => {
                    let (member, selection) = self.key(&method.key, method.computed);
                    let (kind, detail) = match method.kind {
                        MethodDefinitionKind::Constructor => (SymbolKind::CONSTRUCTOR, None),
                        MethodDefinitionKind::Method => (SymbolKind::METHOD, None),
                        MethodDefinitionKind::Get => (SymbolKind::PROPERTY, Some("get")),
                        MethodDefinitionKind::Set => (SymbolKind::PROPERTY, Some("set")),
                    };
                    children.push(Symbol {
                        name: member,
                        detail: detail.map(str::to_owned),
                        kind,
                        range: range(method.span),
                        selection,
                        children: method
                            .value
                            .body
                            .as_ref()
                            .map(|body| self.nested(&body.statements))
                            .unwrap_or_default(),
                    });
                }
                ClassElement::PropertyDefinition(property) => {
                    let (member, selection) = self.key(&property.key, property.computed);
                    children.push(Symbol {
                        name: member,
                        detail: property
                            .type_annotation
                            .as_ref()
                            .map(|a| self.slice(a.type_annotation.span()).to_owned()),
                        kind: SymbolKind::PROPERTY,
                        range: range(property.span),
                        selection,
                        children: Vec::new(),
                    });
                }
                ClassElement::AccessorProperty(property) => {
                    let (member, selection) = self.key(&property.key, property.computed);
                    children.push(Symbol {
                        name: member,
                        detail: Some("accessor".to_owned()),
                        kind: SymbolKind::PROPERTY,
                        range: range(property.span),
                        selection,
                        children: Vec::new(),
                    });
                }
                ClassElement::StaticBlock(_) | ClassElement::TSIndexSignature(_) => {}
            }
        }
        Symbol {
            name,
            detail: None,
            kind: SymbolKind::CLASS,
            range: whole,
            selection,
            children,
        }
    }

    fn interface(
        &self,
        interface: &oxc_ast::ast::TSInterfaceDeclaration<'_>,
        whole: Range32,
    ) -> Symbol {
        let mut children = Vec::new();
        for signature in &interface.body.body {
            match signature {
                TSSignature::TSPropertySignature(property) => {
                    let (name, selection) = self.key(&property.key, property.computed);
                    children.push(Symbol {
                        name,
                        detail: property
                            .type_annotation
                            .as_ref()
                            .map(|a| self.slice(a.type_annotation.span()).to_owned()),
                        kind: SymbolKind::PROPERTY,
                        range: range(property.span),
                        selection,
                        children: Vec::new(),
                    });
                }
                TSSignature::TSMethodSignature(method) => {
                    let (name, selection) = self.key(&method.key, method.computed);
                    children.push(Symbol {
                        name,
                        detail: None,
                        kind: SymbolKind::METHOD,
                        range: range(method.span),
                        selection,
                        children: Vec::new(),
                    });
                }
                TSSignature::TSCallSignatureDeclaration(call) => children.push(Symbol {
                    name: "()".to_owned(),
                    detail: None,
                    kind: SymbolKind::METHOD,
                    range: range(call.span),
                    selection: range(call.span),
                    children: Vec::new(),
                }),
                TSSignature::TSConstructSignatureDeclaration(construct) => children.push(Symbol {
                    name: "new()".to_owned(),
                    detail: None,
                    kind: SymbolKind::CONSTRUCTOR,
                    range: range(construct.span),
                    selection: range(construct.span),
                    children: Vec::new(),
                }),
                TSSignature::TSIndexSignature(index) => children.push(Symbol {
                    name: "[]".to_owned(),
                    detail: Some(self.slice(index.span).to_owned()),
                    kind: SymbolKind::PROPERTY,
                    range: range(index.span),
                    selection: range(index.span),
                    children: Vec::new(),
                }),
            }
        }
        Symbol {
            name: interface.id.name.to_string(),
            detail: None,
            kind: SymbolKind::INTERFACE,
            range: whole,
            selection: range(interface.id.span),
            children,
        }
    }

    /// A member's name as written, and where. A computed name keeps its brackets.
    fn key(&self, key: &PropertyKey<'_>, computed: bool) -> (String, Range32) {
        let span = key.span();
        let name = match key {
            PropertyKey::StaticIdentifier(id) if !computed => id.name.to_string(),
            PropertyKey::PrivateIdentifier(id) => format!("#{}", id.name),
            _ if computed => format!("[{}]", self.slice(span)),
            _ => self.slice(span).to_owned(),
        };
        (name, range(span))
    }
}

/// The outline as the nested form of the protocol. A range always holds its name's range,
/// which the protocol requires and an empty declaration (a default export with no name)
/// could otherwise break.
pub fn to_nested(symbols: &[Symbol], text: &str, map: &LineMap) -> Vec<DocumentSymbol> {
    symbols
        .iter()
        .map(|symbol| {
            let start = symbol.range.0.min(symbol.selection.0);
            let end = symbol.range.1.max(symbol.selection.1);
            #[allow(deprecated)]
            DocumentSymbol {
                name: symbol.name.clone(),
                detail: symbol.detail.clone(),
                kind: symbol.kind,
                tags: None,
                deprecated: None,
                range: map.range(text, start as usize, end as usize),
                selection_range: map.range(
                    text,
                    symbol.selection.0 as usize,
                    symbol.selection.1 as usize,
                ),
                children: if symbol.children.is_empty() {
                    None
                } else {
                    Some(to_nested(&symbol.children, text, map))
                },
            }
        })
        .collect()
}

/// The outline as the flat form, for a client that does not read the nested one: every
/// symbol once, a member naming its container.
pub fn to_flat(symbols: &[Symbol], text: &str, map: &LineMap, uri: &Uri) -> Vec<SymbolInformation> {
    fn walk(
        symbols: &[Symbol],
        container: Option<&str>,
        text: &str,
        map: &LineMap,
        uri: &Uri,
        out: &mut Vec<SymbolInformation>,
    ) {
        for symbol in symbols {
            let start = symbol.range.0.min(symbol.selection.0);
            let end = symbol.range.1.max(symbol.selection.1);
            #[allow(deprecated)]
            out.push(SymbolInformation {
                name: symbol.name.clone(),
                kind: symbol.kind,
                tags: None,
                deprecated: None,
                location: Location {
                    uri: uri.clone(),
                    range: map.range(text, start as usize, end as usize),
                },
                container_name: container.map(str::to_owned),
            });
            walk(&symbol.children, Some(&symbol.name), text, map, uri, out);
        }
    }
    let mut out = Vec::new();
    walk(symbols, None, text, map, uri, &mut out);
    out
}

/// A symbol found by a workspace search.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub path: PathBuf,
    pub symbol: Symbol,
    /// The symbol that holds it (a class for a method).
    pub container: Option<String>,
}

/// How well `query` matches `name`; lower is better, `None` is no match. Matching ignores case.
/// An empty query matches everything. Exact, then prefix, then contained, then letters in order.
pub fn match_score(query: &str, name: &str) -> Option<u32> {
    if query.is_empty() {
        return Some(1000);
    }
    let query = query.to_lowercase();
    let name = name.to_lowercase();
    if name == query {
        return Some(0);
    }
    if name.starts_with(&query) {
        return Some(1);
    }
    if let Some(at) = name.find(&query) {
        return Some(2 + at as u32);
    }
    // The letters of the query, in order, anywhere: the closer together the better.
    let mut remaining = query.chars();
    let mut want = remaining.next()?;
    let mut first = None;
    for (index, ch) in name.chars().enumerate() {
        if ch == want {
            first.get_or_insert(index);
            match remaining.next() {
                Some(next) => want = next,
                None => return Some(100 + (index - first.unwrap_or(0)) as u32),
            }
        }
    }
    None
}

/// The declarations in `files` whose names match `query`, best first, at most `limit`. Stops
/// between files when `cancel` is set, returning what was found.
pub fn workspace_symbols(
    host: &dyn Host,
    files: &[PathBuf],
    query: &str,
    limit: usize,
    cancel: &AtomicBool,
) -> Vec<Found> {
    fn walk(
        symbols: &[Symbol],
        container: Option<&str>,
        path: &Path,
        query: &str,
        out: &mut Vec<(u32, Found)>,
    ) {
        for symbol in symbols {
            if let Some(score) = match_score(query, &symbol.name) {
                out.push((
                    score,
                    Found {
                        path: path.to_path_buf(),
                        symbol: Symbol {
                            children: Vec::new(),
                            ..symbol.clone()
                        },
                        container: container.map(str::to_owned),
                    },
                ));
            }
            walk(&symbol.children, Some(&symbol.name), path, query, out);
        }
    }

    let mut scored: Vec<(u32, Found)> = Vec::new();
    for file in files {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let Some(text) = host.text(file) else {
            continue;
        };
        let name = file
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if let Some(outline) = outline(&text, &name) {
            walk(&outline, None, file, query, &mut scored);
        }
    }
    scored.sort_by(|(a_score, a), (b_score, b)| {
        (a_score, a.symbol.name.len(), &a.path, a.symbol.selection.0).cmp(&(
            b_score,
            b.symbol.name.len(),
            &b.path,
            b.symbol.selection.0,
        ))
    });
    scored.truncate(limit);
    scored.into_iter().map(|(_, found)| found).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(symbols: &[Symbol]) -> Vec<(&str, SymbolKind)> {
        symbols.iter().map(|s| (s.name.as_str(), s.kind)).collect()
    }

    fn of(text: &str) -> Vec<Symbol> {
        outline(text, "a.ts").unwrap()
    }

    #[test]
    fn top_level_declarations_in_source_order() {
        let symbols = of(
            "function f() {}\nclass C {}\ninterface I {}\ntype T = number;\nenum E { A }\nconst k = 1;\nlet v = 2;\n",
        );
        assert_eq!(
            names(&symbols),
            vec![
                ("f", SymbolKind::FUNCTION),
                ("C", SymbolKind::CLASS),
                ("I", SymbolKind::INTERFACE),
                ("T", SymbolKind::TYPE_PARAMETER),
                ("E", SymbolKind::ENUM),
                ("k", SymbolKind::CONSTANT),
                ("v", SymbolKind::VARIABLE),
            ]
        );
    }

    #[test]
    fn selection_is_the_name_and_range_is_the_whole_declaration() {
        let text = "export function greet(name: string) { return name; }\n";
        let symbols = of(text);
        let symbol = &symbols[0];
        assert_eq!(
            &text[symbol.selection.0 as usize..symbol.selection.1 as usize],
            "greet"
        );
        assert_eq!(symbol.range.0, 0, "the range starts at `export`");
        assert_eq!(
            &text[symbol.range.0 as usize..symbol.range.1 as usize],
            text.trim_end()
        );
    }

    #[test]
    fn class_members_by_kind() {
        let symbols = of(
            "class C {\n  x: number = 1;\n  #p = 2;\n  constructor() {}\n  m() {}\n  get g() { return 1; }\n  set g(v) {}\n  static s() {}\n  [key]() {}\n  accessor a = 1;\n}\n",
        );
        let members = &symbols[0].children;
        assert_eq!(
            members
                .iter()
                .map(|m| (m.name.as_str(), m.kind))
                .collect::<Vec<_>>(),
            vec![
                ("x", SymbolKind::PROPERTY),
                ("#p", SymbolKind::PROPERTY),
                ("constructor", SymbolKind::CONSTRUCTOR),
                ("m", SymbolKind::METHOD),
                ("g", SymbolKind::PROPERTY),
                ("g", SymbolKind::PROPERTY),
                ("s", SymbolKind::METHOD),
                ("[key]", SymbolKind::METHOD),
                ("a", SymbolKind::PROPERTY),
            ]
        );
        assert_eq!(members[0].detail.as_deref(), Some("number"));
        assert_eq!(members[4].detail.as_deref(), Some("get"));
        assert_eq!(members[5].detail.as_deref(), Some("set"));
    }

    #[test]
    fn interface_members() {
        let symbols = of(
            "interface I { a: string; b?(): void; (x: number): void; new (): I; [k: string]: any }",
        );
        let kinds: Vec<_> = symbols[0]
            .children
            .iter()
            .map(|m| (m.name.as_str(), m.kind))
            .collect();
        assert_eq!(
            kinds,
            vec![
                ("a", SymbolKind::PROPERTY),
                ("b", SymbolKind::METHOD),
                ("()", SymbolKind::METHOD),
                ("new()", SymbolKind::CONSTRUCTOR),
                ("[]", SymbolKind::PROPERTY),
            ]
        );
    }

    #[test]
    fn enum_members_and_const_enum() {
        let symbols = of("const enum E { A = 1, 'b-c' = 2 }");
        assert_eq!(symbols[0].detail.as_deref(), Some("const"));
        let members: Vec<_> = symbols[0]
            .children
            .iter()
            .map(|m| m.name.as_str())
            .collect();
        assert_eq!(members, vec!["A", "'b-c'"]);
    }

    #[test]
    fn namespaces_modules_and_global() {
        let symbols = of(
            "namespace A.B.C { export const x = 1; }\ndeclare module 'm' { function f(): void; }\ndeclare global { interface W {} }\n",
        );
        assert_eq!(
            names(&symbols),
            vec![
                ("A.B.C", SymbolKind::NAMESPACE),
                ("'m'", SymbolKind::MODULE),
                ("global", SymbolKind::NAMESPACE),
            ]
        );
        assert_eq!(
            names(&symbols[0].children),
            vec![("x", SymbolKind::CONSTANT)]
        );
        assert_eq!(
            names(&symbols[1].children),
            vec![("f", SymbolKind::FUNCTION)]
        );
        assert_eq!(
            names(&symbols[2].children),
            vec![("W", SymbolKind::INTERFACE)]
        );
    }

    #[test]
    fn destructuring_names_each_binding_and_arrow_functions_are_functions() {
        let symbols = of(
            "const { a, b: { c }, ...rest } = o;\nconst [d, , e = 1] = arr;\nconst go = () => 1;\n",
        );
        assert_eq!(
            names(&symbols),
            vec![
                ("a", SymbolKind::CONSTANT),
                ("c", SymbolKind::CONSTANT),
                ("rest", SymbolKind::CONSTANT),
                ("d", SymbolKind::CONSTANT),
                ("e", SymbolKind::CONSTANT),
                ("go", SymbolKind::FUNCTION),
            ]
        );
    }

    #[test]
    fn several_declarators_each_get_their_own_range() {
        let text = "let a = 1, b = 2;";
        let symbols = of(text);
        assert_eq!(symbols.len(), 2);
        assert_eq!(
            &text[symbols[1].range.0 as usize..symbols[1].range.1 as usize],
            "b = 2"
        );
    }

    #[test]
    fn default_exports_are_named_default_or_by_their_own_name() {
        let symbols = of("export default function () {}\n");
        assert_eq!(names(&symbols), vec![("default", SymbolKind::FUNCTION)]);
        let symbols = of("export default class K {}\n");
        assert_eq!(names(&symbols), vec![("K", SymbolKind::CLASS)]);
        assert!(of("export default 1 + 2;\n").is_empty());
    }

    #[test]
    fn nested_functions_and_classes_are_children_but_locals_are_not() {
        let symbols = of("function outer() { const local = 1; function inner() {} class K {} }");
        assert_eq!(
            names(&symbols[0].children),
            vec![("inner", SymbolKind::FUNCTION), ("K", SymbolKind::CLASS)]
        );
    }

    #[test]
    fn a_file_with_a_syntax_error_still_has_the_declarations_before_it() {
        let symbols = of("function good() {}\nclass Broken {\n  m( {\n");
        assert!(symbols.iter().any(|s| s.name == "good"), "got {symbols:?}");
    }

    #[test]
    fn an_unclosed_block_still_outlines_what_came_before_it() {
        let text = "export function a() {}\nclass Half {\n  m() {\n";
        let symbols = of(text);
        assert_eq!(names(&symbols), vec![("a", SymbolKind::FUNCTION)]);
        // Offsets are into the whole text, not the cut one.
        assert_eq!(
            &text[symbols[0].selection.0 as usize..symbols[0].selection.1 as usize],
            "a"
        );
    }

    #[test]
    fn a_file_that_is_only_an_error_has_an_empty_outline() {
        assert!(of("class {{{").is_empty());
        assert!(of("}}}").is_empty());
    }

    #[test]
    fn dialect_follows_the_extension() {
        let symbols = outline(
            "const el = <div />;\nfunction C() { return el; }\n",
            "a.tsx",
        )
        .unwrap();
        assert_eq!(
            names(&symbols),
            vec![("el", SymbolKind::CONSTANT), ("C", SymbolKind::FUNCTION)]
        );
        let symbols = outline("function f() {}\n", "a.js").unwrap();
        assert_eq!(names(&symbols), vec![("f", SymbolKind::FUNCTION)]);
    }

    #[test]
    fn an_empty_file_has_an_empty_outline() {
        assert!(of("").is_empty());
    }

    #[test]
    fn a_file_nested_past_the_limit_is_refused() {
        let text = "(".repeat(tsox_base::stack_guard::MAX_BRACKET_DEPTH + 10);
        assert_eq!(outline(&text, "a.ts"), None);
    }

    #[test]
    fn positions_are_utf16_in_the_nested_form() {
        let text = "const s = '\u{1F600}'; function after() {}\n";
        let symbols = of(text);
        let map = LineMap::new(text);
        let nested = to_nested(&symbols, text, &map);
        let after = nested.iter().find(|s| s.name == "after").unwrap();
        let byte = text.find("after").unwrap();
        assert_eq!(after.selection_range.start.character as usize, byte - 2);
    }

    #[test]
    fn the_flat_form_names_each_container() {
        let text = "class C { m() {} }\n";
        let symbols = of(text);
        let map = LineMap::new(text);
        let uri: Uri = "file:///a.ts".parse().unwrap();
        let flat = to_flat(&symbols, text, &map, &uri);
        assert_eq!(flat.len(), 2);
        assert_eq!(flat[0].container_name, None);
        assert_eq!(flat[1].name, "m");
        assert_eq!(flat[1].container_name.as_deref(), Some("C"));
    }

    #[test]
    fn a_range_always_holds_its_selection() {
        let text = "export default function () {}\n";
        let symbols = of(text);
        let map = LineMap::new(text);
        for symbol in to_nested(&symbols, text, &map) {
            let (r, s) = (symbol.range, symbol.selection_range);
            assert!(
                (r.start.line, r.start.character) <= (s.start.line, s.start.character)
                    && (r.end.line, r.end.character) >= (s.end.line, s.end.character)
            );
        }
    }

    struct Files(std::collections::HashMap<PathBuf, std::sync::Arc<str>>);

    impl Host for Files {
        fn text(&self, path: &Path) -> Option<std::sync::Arc<str>> {
            self.0.get(path).cloned()
        }

        fn resolve(&self, _: &Path, _: &str) -> Option<PathBuf> {
            None
        }
    }

    fn files(list: &[(&str, &str)]) -> (Files, Vec<PathBuf>) {
        let map: std::collections::HashMap<_, _> = list
            .iter()
            .map(|(name, text)| {
                (
                    PathBuf::from(format!("/p/{name}")),
                    std::sync::Arc::from(*text),
                )
            })
            .collect();
        let mut paths: Vec<_> = map.keys().cloned().collect();
        paths.sort();
        (Files(map), paths)
    }

    #[test]
    fn matching_prefers_exact_then_prefix_then_contained_then_scattered() {
        assert_eq!(match_score("box", "Box"), Some(0));
        assert_eq!(match_score("bo", "Box"), Some(1));
        assert!(match_score("ox", "Box") > Some(1));
        assert!(match_score("bx", "Box") > match_score("ox", "Box"));
        assert_eq!(match_score("zz", "Box"), None);
        assert_eq!(match_score("", "anything"), Some(1000));
        assert!(match_score("gb", "getBox").is_some(), "letters in order");
        assert_eq!(match_score("bg", "getBox"), None, "but in order");
    }

    #[test]
    fn the_workspace_is_searched_across_files_best_first() {
        let (host, paths) = files(&[
            (
                "a.ts",
                "export class Box { open() {} }\nexport const boxes = [];\n",
            ),
            ("b.ts", "export function openBox() {}\ninterface Boxy {}\n"),
        ]);
        let found = workspace_symbols(&host, &paths, "box", 50, &AtomicBool::new(false));
        let names: Vec<&str> = found.iter().map(|f| f.symbol.name.as_str()).collect();
        assert_eq!(names, vec!["Box", "Boxy", "boxes", "openBox"]);
        assert_eq!(found[0].path, PathBuf::from("/p/a.ts"));
    }

    #[test]
    fn members_carry_their_container() {
        let (host, paths) = files(&[("a.ts", "class Box { open() {} close() {} }\n")]);
        let found = workspace_symbols(&host, &paths, "open", 50, &AtomicBool::new(false));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].container.as_deref(), Some("Box"));
        assert_eq!(found[0].symbol.kind, SymbolKind::METHOD);
    }

    #[test]
    fn the_limit_and_cancellation_are_respected() {
        let (host, paths) = files(&[("a.ts", "const a1 = 1; const a2 = 2; const a3 = 3;\n")]);
        assert_eq!(
            workspace_symbols(&host, &paths, "a", 2, &AtomicBool::new(false)).len(),
            2
        );
        assert!(workspace_symbols(&host, &paths, "a", 10, &AtomicBool::new(true)).is_empty());
    }

    #[test]
    fn an_empty_query_lists_everything_up_to_the_limit() {
        let (host, paths) = files(&[("a.ts", "const x = 1;\nfunction y() {}\n")]);
        assert_eq!(
            workspace_symbols(&host, &paths, "", 10, &AtomicBool::new(false)).len(),
            2
        );
    }
}
