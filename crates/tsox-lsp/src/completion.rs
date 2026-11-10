//! Completion.
//!
//! Completion is asked for while the code is being typed, so the file in front of it is rarely
//! valid. Two things make that workable. A small scanner reads the text before the cursor to
//! learn where it is (in a string, in a comment, inside which brackets) without a parser. And
//! to find what is in scope, a placeholder name is put at the cursor and the file read as it
//! is; if the parser still cannot read it, the text up to the cursor is closed off with the
//! brackets it left open and read instead.
//!
//! What it offers, by where the cursor is:
//!
//! * in a module specifier (`from '|'`, `import('|')`, `require('|')`): the files and folders
//!   next to the file, or the packages in the nearest `node_modules`;
//! * after `name.` where `name` is a namespace import or an enum: its exports or its members;
//! * anywhere else a name can be written: what is in scope there, innermost first, then the
//!   keywords;
//! * where a new name is being declared (`const |`), nothing.
//!
//! What it cannot offer is the members of a value whose type has to be worked out
//! (`object.|`): that needs the checker to list members, which it does not yet.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use oxc_ast::AstKind;
use oxc_ast::ast::{Declaration, ModuleDeclaration};
use oxc_semantic::Semantic;

use crate::analysis::with_program;
use crate::hover;
use crate::navigate::{self, Host, declared_names, with_semantic};

/// What kind of thing a completion is, independent of any protocol.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Function,
    Class,
    Interface,
    TypeAlias,
    Enum,
    EnumMember,
    Variable,
    Constant,
    Namespace,
    Module,
    Property,
    Keyword,
    File,
    Folder,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub label: String,
    pub kind: Kind,
    pub detail: Option<String>,
    /// Orders items that the client would otherwise order by label.
    pub sort: String,
}

// ------------------------------------------------------------------------------------------
// The scanner

/// An open bracket, and how many commas have been written inside it at its own level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Open {
    pub ch: char,
    pub at: usize,
    pub commas: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Context {
    Code,
    /// In a string or template literal that began at `start`.
    String {
        quote: char,
        start: usize,
    },
    Comment,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Scan {
    pub context: Context,
    pub open: Vec<Open>,
}

/// Reads `prefix` (the text before the cursor) for where its end is. Regular-expression
/// literals and `${}` inside templates are not understood; a slash or a quote inside them can
/// mislead it, which at worst gives no completion.
pub(crate) fn scan(prefix: &str) -> Scan {
    let mut open: Vec<Open> = Vec::new();
    let mut chars = prefix.char_indices().peekable();
    let mut context = Context::Code;
    while let Some((index, ch)) = chars.next() {
        match context {
            Context::Code => match ch {
                '/' => match chars.peek().map(|(_, next)| *next) {
                    Some('/') => {
                        chars.next();
                        // A line comment lasts to the end of the line.
                        let mut ended = false;
                        for (_, c) in chars.by_ref() {
                            if c == '\n' {
                                ended = true;
                                break;
                            }
                        }
                        context = if ended {
                            Context::Code
                        } else {
                            Context::Comment
                        };
                    }
                    Some('*') => {
                        chars.next();
                        let mut previous = '\0';
                        let mut ended = false;
                        for (_, c) in chars.by_ref() {
                            if previous == '*' && c == '/' {
                                ended = true;
                                break;
                            }
                            previous = c;
                        }
                        context = if ended {
                            Context::Code
                        } else {
                            Context::Comment
                        };
                    }
                    _ => {}
                },
                '"' | '\'' | '`' => {
                    context = Context::String {
                        quote: ch,
                        start: index,
                    }
                }
                '(' | '[' | '{' => open.push(Open {
                    ch,
                    at: index,
                    commas: 0,
                }),
                ')' | ']' | '}' => {
                    let wanted = match ch {
                        ')' => '(',
                        ']' => '[',
                        _ => '{',
                    };
                    if open.last().is_some_and(|top| top.ch == wanted) {
                        open.pop();
                    }
                }
                ',' => {
                    if let Some(top) = open.last_mut() {
                        top.commas += 1;
                    }
                }
                _ => {}
            },
            Context::String { quote, .. } => match ch {
                '\\' => {
                    chars.next();
                }
                c if c == quote => context = Context::Code,
                // A quote that is not a template ends at the end of its line.
                '\n' if quote != '`' => context = Context::Code,
                _ => {}
            },
            Context::Comment => {}
        }
    }
    Scan { context, open }
}

fn closer(open: char) -> char {
    match open {
        '(' => ')',
        '[' => ']',
        _ => '}',
    }
}

/// The text before the cursor with `filler` at the cursor and every bracket left open closed:
/// a file that can be read, up to the cursor.
pub(crate) fn closed_prefix(text: &str, offset: usize, filler: &str) -> String {
    let prefix = &text[..offset];
    let mut out = String::with_capacity(prefix.len() + filler.len() + 8);
    out.push_str(prefix);
    out.push_str(filler);
    for open in scan(prefix).open.iter().rev() {
        out.push(closer(open.ch));
    }
    out
}

fn is_ident_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_' || ch == '$'
}

/// Where the identifier being typed at the end of `prefix` starts.
fn word_start(prefix: &str) -> usize {
    prefix
        .char_indices()
        .rev()
        .take_while(|(_, ch)| is_ident_char(*ch))
        .last()
        .map_or(prefix.len(), |(index, _)| index)
}

// ------------------------------------------------------------------------------------------
// Names in scope

const FILLER: &str = "tsox_cursor__";

const KEYWORDS: &[&str] = &[
    "abstract",
    "as",
    "async",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "declare",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "implements",
    "import",
    "in",
    "instanceof",
    "interface",
    "keyof",
    "let",
    "namespace",
    "new",
    "null",
    "of",
    "private",
    "protected",
    "public",
    "readonly",
    "return",
    "satisfies",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "type",
    "typeof",
    "var",
    "void",
    "while",
    "yield",
];

/// Words after which what is written is a new name, not a use of one.
const DECLARING: &[&str] = &[
    "const",
    "let",
    "var",
    "function",
    "class",
    "interface",
    "type",
    "enum",
    "namespace",
    "module",
];

fn kind_of(
    semantic: &Semantic<'_>,
    node: oxc_semantic::NodeId,
    text: &str,
    name: &str,
) -> (Kind, Option<String>) {
    let ann = |a: &Option<oxc_allocator::Box<'_, oxc_ast::ast::TSTypeAnnotation<'_>>>| {
        a.as_ref().map(|a| {
            text.get(a.type_annotation.span().start as usize..a.type_annotation.span().end as usize)
                .unwrap_or("")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
    };
    use oxc_span::GetSpan;
    match semantic.nodes().kind(node) {
        AstKind::Function(function) => (
            Kind::Function,
            Some(hover::function_signature(function, text, name).0),
        ),
        AstKind::Class(class) => (Kind::Class, Some(hover::class_header(class, text))),
        AstKind::TSInterfaceDeclaration(interface) => (
            Kind::Interface,
            Some(hover::interface_header(interface, text)),
        ),
        AstKind::TSTypeAliasDeclaration(alias) => {
            (Kind::TypeAlias, Some(hover::alias_header(alias, text)))
        }
        AstKind::TSEnumDeclaration(declaration) => {
            (Kind::Enum, Some(hover::enum_header(declaration)))
        }
        AstKind::TSNamespaceDeclaration(_) => (Kind::Namespace, None),
        AstKind::VariableDeclarator(declarator) => {
            let constant = semantic
                .nodes()
                .ancestors(node)
                .find_map(|ancestor| match ancestor.kind() {
                    AstKind::VariableDeclaration(declaration) => {
                        Some(declaration.kind.as_str() == "const")
                    }
                    _ => None,
                })
                .unwrap_or(false);
            let callable = matches!(
                declarator.init,
                Some(
                    oxc_ast::ast::Expression::ArrowFunctionExpression(_)
                        | oxc_ast::ast::Expression::FunctionExpression(_)
                )
            );
            let kind = if callable {
                Kind::Function
            } else if constant {
                Kind::Constant
            } else {
                Kind::Variable
            };
            (kind, ann(&declarator.type_annotation))
        }
        AstKind::FormalParameter(parameter) => (Kind::Variable, ann(&parameter.type_annotation)),
        AstKind::ImportNamespaceSpecifier(_) => (Kind::Module, None),
        _ => (Kind::Variable, None),
    }
}

/// The names visible where the filler sits in `text`, innermost scope first.
fn names_in_scope(path: &Path, text: &str, offset: u32) -> Option<Vec<Item>> {
    with_semantic(path, text, |_, semantic| {
        let scoping = semantic.scoping();
        let node = semantic.nodes().iter().find(|node| match node.kind() {
            AstKind::IdentifierReference(reference) => {
                reference.name.as_str().ends_with(FILLER)
                    && reference.span.start <= offset
                    && offset <= reference.span.end
            }
            _ => false,
        })?;
        let mut scope = Some(node.scope_id());
        let mut depth = 0u32;
        let mut seen: HashSet<String> = HashSet::new();
        let mut items = Vec::new();
        while let Some(id) = scope {
            for (name, symbol) in scoping.get_bindings(id) {
                let name = name.as_str();
                if name.ends_with(FILLER) || !seen.insert(name.to_owned()) {
                    continue;
                }
                let declaration = scoping.symbol_declaration(*symbol);
                let (kind, detail) = kind_of(semantic, declaration, text, name);
                items.push(Item {
                    label: name.to_owned(),
                    kind,
                    detail,
                    sort: format!("0{depth:02}_{name}"),
                });
            }
            depth += 1;
            scope = scoping.scope_parent_id(id);
        }
        Some(items)
    })
    .flatten()
}

fn scope_items(path: &Path, text: &str, offset: usize) -> Vec<Item> {
    // First the file as it is with the filler in it; if the parser cannot read that, the text
    // up to the cursor, closed off.
    let whole = format!("{}{FILLER}{}", &text[..offset], &text[offset..]);
    names_in_scope(path, &whole, offset as u32)
        .or_else(|| {
            let closed = closed_prefix(text, offset, FILLER);
            names_in_scope(path, &closed, offset as u32)
        })
        .unwrap_or_default()
}

// ------------------------------------------------------------------------------------------
// Members of namespace imports and enums

fn declaration_kind(declaration: &Declaration<'_>) -> Kind {
    match declaration {
        Declaration::FunctionDeclaration(_) => Kind::Function,
        Declaration::ClassDeclaration(_) => Kind::Class,
        Declaration::TSInterfaceDeclaration(_) => Kind::Interface,
        Declaration::TSTypeAliasDeclaration(_) => Kind::TypeAlias,
        Declaration::TSEnumDeclaration(_) => Kind::Enum,
        Declaration::TSNamespaceDeclaration(_) | Declaration::TSExternalModuleDeclaration(_) => {
            Kind::Namespace
        }
        Declaration::VariableDeclaration(variables) => {
            if variables.kind.as_str() == "const" {
                Kind::Constant
            } else {
                Kind::Variable
            }
        }
        _ => Kind::Variable,
    }
}

/// What a file exports, following `export * from`.
fn module_exports(
    host: &dyn Host,
    path: &Path,
    depth: usize,
    seen: &mut HashSet<PathBuf>,
) -> Vec<(String, Kind)> {
    if depth > 8 || !seen.insert(path.to_path_buf()) {
        return Vec::new();
    }
    let Some(text) = host.text(path) else {
        return Vec::new();
    };
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut found: Vec<(String, Kind)> = Vec::new();
    let mut stars: Vec<String> = Vec::new();
    with_program(&text, &name, |program| {
        for statement in &program.body {
            match statement.as_module_declaration() {
                Some(ModuleDeclaration::ExportDeclaration(export)) => {
                    let kind = declaration_kind(&export.declaration);
                    for (declared, _) in declared_names(&export.declaration) {
                        found.push((declared, kind));
                    }
                }
                Some(ModuleDeclaration::ExportNamedDeclaration(export)) => {
                    for specifier in &export.specifiers {
                        found.push((specifier.exported.name().to_string(), Kind::Variable));
                    }
                }
                Some(ModuleDeclaration::ExportFromDeclaration(export)) => {
                    for specifier in &export.specifiers {
                        found.push((specifier.exported.name().to_string(), Kind::Variable));
                    }
                }
                Some(ModuleDeclaration::ExportAllDeclaration(export)) => match &export.exported {
                    Some(namespace) => found.push((namespace.name().to_string(), Kind::Module)),
                    None => stars.push(export.source.value.to_string()),
                },
                Some(ModuleDeclaration::ExportDefaultDeclaration(_)) => {
                    found.push(("default".to_owned(), Kind::Variable));
                }
                _ => {}
            }
        }
    });
    for source in stars {
        if let Some(target) = host.resolve(path, &source) {
            found.extend(module_exports(host, &target, depth + 1, seen));
        }
    }
    found
}

/// The members of an enum declared at `span` in `path`.
fn enum_members(host: &dyn Host, path: &Path, span: (u32, u32)) -> Option<Vec<String>> {
    let text = host.text(path)?;
    with_semantic(path, &text, |_, semantic| {
        let binding = semantic.nodes().iter().find_map(|node| match node.kind() {
            AstKind::BindingIdentifier(id) if (id.span.start, id.span.end) == span => Some(id),
            _ => None,
        })?;
        let declaration = semantic
            .scoping()
            .symbol_declaration(binding.symbol_id.get()?);
        match semantic.nodes().kind(declaration) {
            AstKind::TSEnumDeclaration(declaration) => Some(
                declaration
                    .body
                    .members
                    .iter()
                    .map(|member| match &member.id {
                        oxc_ast::ast::TSEnumMemberName::Identifier(id) => id.name.to_string(),
                        other => {
                            use oxc_span::GetSpan;
                            text.get(other.span().start as usize..other.span().end as usize)
                                .unwrap_or("")
                                .to_owned()
                        }
                    })
                    .collect(),
            ),
            _ => None,
        }
    })
    .flatten()
}

/// The members of `object.`, for the two things whose members can be read from syntax.
fn member_items(host: &dyn Host, path: &Path, text: &str, offset: usize, dot: usize) -> Vec<Item> {
    // The identifier just before the dot, and nothing longer: `a.b.` has no answer here.
    let before = &text[..dot];
    let start = word_start(before);
    if start == before.len() || before[..start].trim_end().ends_with('.') {
        return Vec::new();
    }
    let object_end = before.len();
    let probe = object_end - 1;

    // Try the file as it is, then the text up to the cursor closed off.
    let whole = format!("{}{FILLER}{}", &text[..offset], &text[offset..]);
    let mut hits = navigate::definition(host, path, &whole, probe);
    if hits.is_empty() {
        hits = navigate::definition(host, path, &closed_prefix(text, offset, FILLER), probe);
    }
    let mut items = Vec::new();
    for hit in hits {
        if hit.span == (0, 0) {
            let mut seen = HashSet::new();
            for (name, kind) in module_exports(host, &hit.path, 0, &mut seen) {
                items.push(Item {
                    sort: format!("0_{name}"),
                    label: name,
                    kind,
                    detail: None,
                });
            }
        } else if let Some(members) = enum_members(host, &hit.path, hit.span) {
            for name in members {
                items.push(Item {
                    sort: format!("0_{name}"),
                    label: name,
                    kind: Kind::EnumMember,
                    detail: None,
                });
            }
        }
    }
    items.sort_by(|a, b| a.label.cmp(&b.label));
    items.dedup_by(|a, b| a.label == b.label);
    items
}

// ------------------------------------------------------------------------------------------
// Module specifiers

const EXTENSIONS: &[&str] = &[
    ".d.ts", ".tsx", ".mts", ".cts", ".ts", ".jsx", ".mjs", ".cjs", ".js",
];

/// The files, folders and packages a specifier typed so far (`typed`) can continue with.
fn specifier_items(path: &Path, typed: &str) -> Vec<Item> {
    let mut items: Vec<Item> = Vec::new();
    let mut push = |label: String, kind: Kind| {
        if !items.iter().any(|item| item.label == label) {
            items.push(Item {
                sort: format!("0_{label}"),
                label,
                kind,
                detail: None,
            });
        }
    };
    let Some(here) = path.parent() else {
        return items;
    };
    if typed.starts_with('.') {
        let (directory_part, name_prefix) = match typed.rfind('/') {
            Some(at) => (&typed[..=at], &typed[at + 1..]),
            None => ("", typed),
        };
        if directory_part.is_empty() {
            // `.` and `..` themselves: the relative starts.
            push("./".to_owned(), Kind::Folder);
            push("../".to_owned(), Kind::Folder);
            items.sort_by(|a, b| a.label.cmp(&b.label));
            return items;
        }
        let directory = here.join(directory_part);
        let Ok(entries) = std::fs::read_dir(&directory) else {
            return items;
        };
        let own_name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || name == "node_modules" || !name.starts_with(name_prefix) {
                continue;
            }
            let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
            if is_dir {
                push(name, Kind::Folder);
            } else if Some(&name) != own_name.as_ref() {
                if let Some(stem) = EXTENSIONS
                    .iter()
                    .find_map(|extension| name.strip_suffix(extension))
                {
                    push(stem.to_owned(), Kind::File);
                } else if name.ends_with(".json") {
                    push(name, Kind::File);
                }
            }
        }
    } else {
        // A package: what the nearest `node_modules` holds.
        let modules = here
            .ancestors()
            .map(|dir| dir.join("node_modules"))
            .find(|candidate| candidate.is_dir());
        push("./".to_owned(), Kind::Folder);
        push("../".to_owned(), Kind::Folder);
        if let Some(modules) = modules {
            let (scope_part, name_prefix) = match typed.rfind('/') {
                Some(at) if typed.starts_with('@') => (&typed[..=at], &typed[at + 1..]),
                _ => ("", typed),
            };
            if let Ok(entries) = std::fs::read_dir(modules.join(scope_part)) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if name.starts_with('.') || name == "@types" || !name.starts_with(name_prefix) {
                        continue;
                    }
                    if name.starts_with('@') && scope_part.is_empty() {
                        push(format!("{name}/"), Kind::Folder);
                    } else {
                        push(name, Kind::Module);
                    }
                }
            }
        }
    }
    items.sort_by(|a, b| a.label.cmp(&b.label));
    items
}

/// Whether the string that starts at `quote_at` is the source of an import or a require.
fn is_module_source(text: &str, quote_at: usize) -> bool {
    let before = text[..quote_at].trim_end();
    before.ends_with("from")
        || before.ends_with("import")
        || before.ends_with("import(")
        || before.ends_with("require(")
        || before.ends_with("import type")
}

// ------------------------------------------------------------------------------------------

/// What can be written at `offset` of `text` (the file at `path`).
pub fn completion(host: &dyn Host, path: &Path, text: &str, offset: usize) -> Vec<Item> {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    let prefix = &text[..offset];
    let scanned = scan(prefix);
    match scanned.context {
        Context::Comment => Vec::new(),
        Context::String { start, .. } => {
            if is_module_source(text, start) {
                specifier_items(path, &prefix[start + 1..])
            } else {
                Vec::new()
            }
        }
        Context::Code => {
            let start = word_start(prefix);
            let before = prefix[..start].trim_end();
            if before.ends_with('.') && !before.ends_with("..") {
                let dot = before.len() - 1;
                return member_items(host, path, text, offset, dot);
            }
            // After a word that begins a declaration, the name is new.
            let previous_word: String = before
                .chars()
                .rev()
                .take_while(|c| is_ident_char(*c))
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            if DECLARING.contains(&previous_word.as_str()) {
                return Vec::new();
            }
            // A number is being written.
            if prefix[start..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_digit())
            {
                return Vec::new();
            }
            let mut items = scope_items(path, text, offset);
            let names: HashSet<String> = items.iter().map(|item| item.label.clone()).collect();
            for keyword in KEYWORDS {
                if !names.contains(*keyword) {
                    items.push(Item {
                        label: (*keyword).to_owned(),
                        kind: Kind::Keyword,
                        detail: None,
                        sort: format!("9_{keyword}"),
                    });
                }
            }
            items
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use super::*;

    struct Files(HashMap<PathBuf, Arc<str>>);

    impl Files {
        fn new(list: &[(&str, &str)]) -> Self {
            Self(
                list.iter()
                    .map(|(name, text)| (PathBuf::from(format!("/p/{name}")), Arc::from(*text)))
                    .collect(),
            )
        }
    }

    impl Host for Files {
        fn text(&self, path: &Path) -> Option<Arc<str>> {
            self.0.get(path).cloned()
        }

        fn resolve(&self, from: &Path, specifier: &str) -> Option<PathBuf> {
            let name = specifier.trim_start_matches("./");
            let candidate = from.parent()?.join(format!("{name}.ts"));
            self.0.contains_key(&candidate).then_some(candidate)
        }
    }

    /// The completions at the `|` in `marked`, which is removed.
    fn at(files: &[(&str, &str)], marked: &str) -> Vec<Item> {
        let offset = marked.find('|').expect("a cursor");
        let text = marked.replacen('|', "", 1);
        let mut all: Vec<(&str, String)> =
            files.iter().map(|(n, t)| (*n, (*t).to_owned())).collect();
        all.push(("a.ts", text.clone()));
        let refs: Vec<(&str, &str)> = all.iter().map(|(n, t)| (*n, t.as_str())).collect();
        let host = Files::new(&refs);
        completion(&host, Path::new("/p/a.ts"), &text, offset)
    }

    fn labels(items: &[Item]) -> Vec<&str> {
        items.iter().map(|i| i.label.as_str()).collect()
    }

    fn names_only(items: &[Item]) -> Vec<&str> {
        items
            .iter()
            .filter(|i| i.kind != Kind::Keyword)
            .map(|i| i.label.as_str())
            .collect()
    }

    #[test]
    fn the_scanner_knows_strings_comments_and_open_brackets() {
        let s = scan("foo(a, [1, 2], {x: (");
        assert_eq!(s.context, Context::Code);
        assert_eq!(
            s.open.iter().map(|o| (o.ch, o.commas)).collect::<Vec<_>>(),
            vec![('(', 2), ('{', 0), ('(', 0)]
        );
        assert!(matches!(
            scan("const s = 'abc").context,
            Context::String { quote: '\'', .. }
        ));
        assert_eq!(scan("// note").context, Context::Comment);
        assert_eq!(scan("/* a").context, Context::Comment);
        assert_eq!(scan("/* a */ x").context, Context::Code);
        assert_eq!(scan("// a\nx").context, Context::Code);
        assert_eq!(scan("'a\\'b' x").context, Context::Code, "an escaped quote");
        assert_eq!(scan("'unterminated\nx").context, Context::Code);
        assert!(matches!(
            scan("`a\nb").context,
            Context::String { quote: '`', .. }
        ));
        assert_eq!(
            scan("f(')')").open.len(),
            0,
            "a bracket in a string is text"
        );
    }

    #[test]
    fn the_text_before_the_cursor_is_closed_off() {
        assert_eq!(closed_prefix("f(a, {b: [", 10, "X"), "f(a, {b: [X]})");
        assert_eq!(closed_prefix("let x = 1;", 10, "X"), "let x = 1;X");
    }

    #[test]
    fn names_in_scope_come_innermost_first_then_keywords() {
        let items = at(
            &[],
            "const outer = 1;\nfunction f(param: number) {\n  const inner = 2;\n  |\n}\nfunction later() {}\n",
        );
        let names = names_only(&items);
        assert_eq!(names, vec!["inner", "param", "outer", "f", "later"]);
        assert!(
            items
                .iter()
                .any(|i| i.label == "return" && i.kind == Kind::Keyword)
        );
        let outer = items.iter().find(|i| i.label == "outer").unwrap();
        assert_eq!(outer.kind, Kind::Constant);
        let f = items.iter().find(|i| i.label == "f").unwrap();
        assert_eq!(f.kind, Kind::Function);
        assert_eq!(f.detail.as_deref(), Some("function f(param: number)"));
    }

    #[test]
    fn a_name_in_an_inner_scope_is_not_offered_outside_it() {
        let items = at(&[], "function f() { const hidden = 1; }\n|\n");
        assert!(!names_only(&items).contains(&"hidden"));
        assert!(names_only(&items).contains(&"f"));
    }

    #[test]
    fn a_shadowed_name_is_offered_once() {
        let items = at(&[], "const v = 1;\nfunction f(v: string) { | }\n");
        assert_eq!(names_only(&items).iter().filter(|n| **n == "v").count(), 1);
        let v = items.iter().find(|i| i.label == "v").unwrap();
        assert_eq!(v.detail.as_deref(), Some("string"), "the inner one");
    }

    #[test]
    fn completion_works_in_a_half_typed_call() {
        let items = at(&[], "const total = 1;\nfunction f() {\n  g(total, |\n");
        assert!(
            names_only(&items).contains(&"total"),
            "{:?}",
            names_only(&items)
        );
        let items = at(&[], "const total = 1;\nfunction f() {\n  g({ a: |\n");
        assert!(names_only(&items).contains(&"total"));
    }

    #[test]
    fn a_partial_word_still_offers_everything_for_the_client_to_filter() {
        let items = at(&[], "const counter = 1;\ncou|\n");
        assert!(names_only(&items).contains(&"counter"));
    }

    #[test]
    fn types_are_offered_where_types_are_written() {
        let items = at(&[], "interface Shape {}\ntype Id = number;\nlet s: |;\n");
        let names = names_only(&items);
        assert!(names.contains(&"Shape") && names.contains(&"Id"));
        let shape = items.iter().find(|i| i.label == "Shape").unwrap();
        assert_eq!(shape.kind, Kind::Interface);
    }

    #[test]
    fn nothing_is_offered_where_a_new_name_is_declared() {
        for marked in [
            "const |",
            "let |",
            "function |",
            "class |",
            "interface |",
            "type |",
            "enum |",
        ] {
            assert!(at(&[], marked).is_empty(), "{marked}");
        }
        // But after the name, in the initializer, everything is.
        assert!(!at(&[], "const a = |").is_empty());
    }

    #[test]
    fn nothing_is_offered_in_a_comment_a_plain_string_or_a_number() {
        assert!(at(&[], "const a = 1;\n// comm|").is_empty());
        assert!(at(&[], "/* a |").is_empty());
        assert!(at(&[], "const a = 'te|xt';").is_empty());
        assert!(at(&[], "const a = 12|").is_empty());
    }

    #[test]
    fn a_namespace_import_offers_its_exports() {
        let items = at(
            &[(
                "lib.ts",
                "export function make() {}\nexport const VERSION = 1;\nexport class Box {}\nexport interface Shape {}\nexport { hidden as visible };\nconst hidden = 1;\nexport default 1;\n",
            )],
            "import * as lib from './lib';\nlib.|\n",
        );
        let found: Vec<(&str, Kind)> = items.iter().map(|i| (i.label.as_str(), i.kind)).collect();
        assert_eq!(
            found,
            vec![
                ("Box", Kind::Class),
                ("Shape", Kind::Interface),
                ("VERSION", Kind::Constant),
                ("default", Kind::Variable),
                ("make", Kind::Function),
                ("visible", Kind::Variable),
            ]
        );
    }

    #[test]
    fn exports_reached_through_export_star_are_offered() {
        let items = at(
            &[
                (
                    "barrel.ts",
                    "export * from './inner';\nexport const own = 1;\n",
                ),
                ("inner.ts", "export const deep = 1;\n"),
            ],
            "import * as b from './barrel';\nb.|\n",
        );
        assert_eq!(labels(&items), vec!["deep", "own"]);
    }

    #[test]
    fn an_enum_offers_its_members() {
        let items = at(&[], "enum Color { Red, Green = 2, 'Dark Blue' }\nColor.|\n");
        assert_eq!(labels(&items), vec!["'Dark Blue'", "Green", "Red"]);
        assert!(items.iter().all(|i| i.kind == Kind::EnumMember));
    }

    #[test]
    fn members_of_a_value_are_not_guessed() {
        assert!(at(&[], "const o = { a: 1 };\no.|\n").is_empty());
        assert!(at(&[], "import * as a from './a';\na.b.|\n").is_empty());
    }

    #[test]
    fn module_specifiers_complete_from_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir_all(root.join("sub")).unwrap();
        for name in [
            "a.ts",
            "util.ts",
            "view.tsx",
            "types.d.ts",
            "data.json",
            "notes.txt",
            ".hidden.ts",
        ] {
            std::fs::write(root.join(name), "").unwrap();
        }
        std::fs::write(root.join("sub/deep.ts"), "").unwrap();
        let a = root.join("a.ts");

        let labels_of = |typed: &str| -> Vec<String> {
            specifier_items(&a, typed)
                .into_iter()
                .map(|i| i.label)
                .collect()
        };
        assert_eq!(
            labels_of("./"),
            vec!["data.json", "sub", "types", "util", "view"]
        );
        assert_eq!(labels_of("./u"), vec!["util"]);
        assert_eq!(labels_of("./sub/"), vec!["deep"]);
        assert_eq!(
            labels_of("../"),
            Vec::<String>::new()
                .into_iter()
                .chain(labels_of("../"))
                .collect::<Vec<_>>()
        );
        assert_eq!(labels_of("."), vec!["../", "./"]);
    }

    #[test]
    fn packages_complete_from_the_nearest_node_modules() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir_all(root.join("node_modules/react")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/@scope/pkg")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/@types/node")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/.bin")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        let a = root.join("src/a.ts");
        let labels_of = |typed: &str| -> Vec<String> {
            specifier_items(&a, typed)
                .into_iter()
                .map(|i| i.label)
                .collect()
        };
        assert_eq!(labels_of(""), vec!["../", "./", "@scope/", "react"]);
        assert_eq!(
            labels_of("re"),
            vec!["../", "./", "react"]
                .into_iter()
                .filter(|l| l.starts_with("re") || l.starts_with('.'))
                .map(String::from)
                .collect::<Vec<_>>()
        );
        assert_eq!(labels_of("@scope/"), vec!["../", "./", "pkg"]);
    }

    #[test]
    fn a_string_that_is_a_module_source_completes_specifiers() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::write(root.join("util.ts"), "").unwrap();
        let a = root.join("a.ts");
        let text = "import { x } from './u";
        let host = Files::new(&[]);
        let items = completion(&host, &a, text, text.len());
        assert_eq!(labels(&items), vec!["util"]);
        let text = "const m = require('./u";
        assert_eq!(
            labels(&completion(&host, &a, text, text.len())),
            vec!["util"]
        );
        let text = "const m = await import('./u";
        assert_eq!(
            labels(&completion(&host, &a, text, text.len())),
            vec!["util"]
        );
        let text = "const s = './u";
        assert!(
            completion(&host, &a, text, text.len()).is_empty(),
            "not a module source"
        );
    }
}
