//! What the editor shows when the pointer rests on a name.
//!
//! Two things are put together. From the syntax of the place the name is declared, a one
//! line header the way TypeScript writes it (`function twice(n: number): number`,
//! `class Box<T>`, `(parameter) n: number`) and the JSDoc comment above it. From the checker,
//! the type the name has there: what an inferred `const` or `let` came out as, what a call
//! returns, what an access narrowed to. A name with no checker type (the project is not
//! checked yet, the file is not in it, the checker could not type that place) still gets its
//! header from the syntax.
//!
//! What it does not know is anything that needs a type to be found: a property that is
//! reached by `object.member` has no declaration here, only the type the checker gives the
//! access.

use std::path::Path;

use oxc_ast::AstKind;
use oxc_ast::ast::{
    Class, Function, PropertyKey, TSAccessibility, TSEnumDeclaration, TSInterfaceDeclaration,
    TSNamespaceDeclaration, TSTypeAliasDeclaration,
};
use oxc_semantic::{NodeId, Semantic};
use oxc_span::{GetSpan, Span};
use tsox::TypeAtOffset;

use crate::navigate::{self, Host, with_semantic};

/// What to show, and which text it is about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hover {
    /// The name the pointer is on, as byte offsets into the text it was asked of.
    pub range: (u32, u32),
    /// Markdown: a code block, then the documentation if there is any.
    pub markdown: String,
}

/// A header longer than this is cut, so that a large type alias does not fill the screen.
const MAX_HEADER: usize = 400;

/// What a name is declared as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// `function` with its signature as written, and whether it names a return type.
    Function {
        signature: String,
        returns: bool,
    },
    /// `const`, `let`, `var`, `using`, with the annotation as written.
    Variable {
        keyword: String,
        annotation: Option<String>,
    },
    Parameter {
        annotation: Option<String>,
    },
    /// A header that is complete as written: a class, an interface, an enum, a namespace, a
    /// type alias.
    Header(String),
    Module(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Declared {
    pub(crate) name: String,
    pub(crate) kind: Kind,
    pub(crate) docs: Option<String>,
}

/// The text of a span, with each run of whitespace made one space.
fn flat(text: &str, span: Span) -> String {
    let slice = text
        .get(span.start as usize..span.end as usize)
        .unwrap_or("");
    let mut out = String::with_capacity(slice.len());
    for word in slice.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

fn clip(mut header: String) -> String {
    if header.len() > MAX_HEADER {
        let mut end = MAX_HEADER;
        while !header.is_char_boundary(end) {
            end -= 1;
        }
        header.truncate(end);
        header.push('…');
    }
    header
}

fn key_name(key: &PropertyKey<'_>, text: &str) -> String {
    match key {
        PropertyKey::StaticIdentifier(id) => id.name.to_string(),
        PropertyKey::PrivateIdentifier(id) => format!("#{}", id.name),
        other => flat(text, other.span()),
    }
}

/// Splits `(a: number) => string` into `(a: number)` and `string`: the parameters are read
/// with their brackets, so an arrow inside a parameter's own type does not end them. `None`
/// when the text is not one function type (a union of them, an object type).
pub fn split_signature(type_text: &str) -> Option<(&str, &str)> {
    let bytes = type_text.as_bytes();
    let mut i = 0;
    let mut depth = 0i32;
    // Generic parameters first: `<T>(x: T) => T`.
    if bytes.first() == Some(&b'<') {
        loop {
            match bytes.get(i)? {
                b'<' => depth += 1,
                b'>' => {
                    depth -= 1;
                    if depth == 0 {
                        i += 1;
                        break;
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }
    if bytes.get(i) != Some(&b'(') {
        return None;
    }
    depth = 0;
    loop {
        match bytes.get(i)? {
            b'(' | b'[' | b'{' | b'<' => depth += 1,
            // An arrow's `>` does not close anything.
            b'>' if i > 0 && bytes[i - 1] == b'=' => {}
            b')' | b']' | b'}' | b'>' => {
                depth -= 1;
                if depth == 0 {
                    i += 1;
                    break;
                }
            }
            _ => {}
        }
        i += 1;
    }
    let rest = type_text.get(i..)?;
    let returns = rest.strip_prefix(" => ")?;
    Some((&type_text[..i], returns))
}

/// The header of a function-typed thing, from a `name` and the checker's `type_text`.
pub(crate) fn signature_from_type(name: &str, type_text: &str) -> Option<String> {
    let (parameters, returns) = split_signature(type_text)?;
    Some(format!("{name}{parameters}: {returns}"))
}

// ------------------------------------------------------------------------------------------
// JSDoc

/// A JSDoc comment as markdown: its description, then its tags.
pub fn jsdoc_markdown(raw: &str) -> String {
    let inner = raw
        .trim()
        .strip_prefix("/**")
        .unwrap_or(raw)
        .trim_end_matches("*/");
    let lines: Vec<String> = inner
        .lines()
        .map(|line| {
            let line = line.trim();
            let line = line.strip_prefix('*').unwrap_or(line);
            line.strip_prefix(' ').unwrap_or(line).trim_end().to_owned()
        })
        .collect();

    let mut description: Vec<String> = Vec::new();
    // Each tag: its name, and the lines that belong to it.
    let mut tags: Vec<(String, Vec<String>)> = Vec::new();
    for line in lines {
        if let Some(rest) = line.trim_start().strip_prefix('@') {
            let (name, body) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            tags.push((name.to_owned(), vec![body.trim().to_owned()]));
        } else if let Some((_, body)) = tags.last_mut() {
            body.push(line);
        } else {
            description.push(line);
        }
    }

    let mut out = Vec::new();
    let description = description.join("\n").trim().to_owned();
    if !description.is_empty() {
        out.push(description);
    }
    for (name, body) in tags {
        let joined = body.join("\n");
        let text = joined.trim();
        let rendered = match name.as_str() {
            "param" | "arg" | "argument" | "property" | "prop" => {
                let text = strip_braced_type(text);
                let (param, rest) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
                let rest = rest.trim().trim_start_matches('-').trim();
                if rest.is_empty() {
                    format!("*@{name}* `{param}`")
                } else {
                    format!("*@{name}* `{param}` — {}", one_line(rest))
                }
            }
            "returns" | "return" => format!("*@returns* {}", one_line(strip_braced_type(text))),
            "deprecated" => {
                if text.is_empty() {
                    "**@deprecated**".to_owned()
                } else {
                    format!("**@deprecated** {}", one_line(text))
                }
            }
            "example" => format!("*@example*\n```ts\n{text}\n```"),
            _ if text.is_empty() => format!("*@{name}*"),
            _ => format!("*@{name}* {}", one_line(text)),
        };
        out.push(rendered);
    }
    out.join("\n\n")
}

fn strip_braced_type(text: &str) -> &str {
    let text = text.trim_start();
    if !text.starts_with('{') {
        return text;
    }
    let mut depth = 0;
    for (i, ch) in text.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return text[i + 1..].trim_start();
                }
            }
            _ => {}
        }
    }
    text
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The JSDoc comment written immediately before any of the starts, as markdown.
fn docs_before(semantic: &Semantic<'_>, text: &str, starts: &[u32]) -> Option<String> {
    let comments = semantic.comments();
    for start in starts {
        let found = comments
            .iter()
            .find(|comment| comment.is_jsdoc() && comment.attached_to == *start);
        if let Some(comment) = found {
            let raw = text.get(comment.span.start as usize..comment.span.end as usize)?;
            let markdown = jsdoc_markdown(raw);
            if !markdown.is_empty() {
                return Some(markdown);
            }
        }
    }
    None
}

/// Where a declaration's own comment may be attached: its start, and the start of what holds
/// it up to the statement (`const a` in `export const a`, whose comment is above `export`).
fn doc_starts(semantic: &Semantic<'_>, node: NodeId) -> Vec<u32> {
    let mut starts = vec![semantic.nodes().kind(node).span().start];
    for ancestor in semantic.nodes().ancestors(node).take(4) {
        if matches!(
            ancestor.kind(),
            AstKind::Program(_) | AstKind::BlockStatement(_) | AstKind::FunctionBody(_)
        ) {
            break;
        }
        starts.push(ancestor.kind().span().start);
    }
    starts.sort_unstable();
    starts.dedup();
    starts
}

// ------------------------------------------------------------------------------------------
// Declarations

pub(crate) fn function_signature(
    function: &Function<'_>,
    text: &str,
    name: &str,
) -> (String, bool) {
    let params_end = function.params.span.end;
    let (end, returns) = match &function.return_type {
        Some(annotation) => (annotation.span.end.max(params_end), true),
        None => (params_end, false),
    };
    let start = function
        .type_parameters
        .as_ref()
        .map_or(function.params.span.start, |generics| generics.span.start);
    let rest = flat(text, Span::new(start, end));
    let mut prefix = String::new();
    if function.r#async {
        prefix.push_str("async ");
    }
    prefix.push_str(if function.generator {
        "function* "
    } else {
        "function "
    });
    (format!("{prefix}{name}{rest}"), returns)
}

pub(crate) fn class_header(class: &Class<'_>, text: &str) -> String {
    let mut header = flat(text, Span::new(class.span.start, class.body.span.start));
    // A class written `export default class` keeps only what the class itself says.
    for prefix in ["export default ", "export ", "declare "] {
        if let Some(rest) = header.strip_prefix(prefix) {
            header = rest.to_owned();
        }
    }
    header
}

pub(crate) fn interface_header(interface: &TSInterfaceDeclaration<'_>, text: &str) -> String {
    flat(
        text,
        Span::new(interface.span.start, interface.body.span.start),
    )
}

pub(crate) fn alias_header(alias: &TSTypeAliasDeclaration<'_>, text: &str) -> String {
    flat(text, alias.span)
        .trim_end_matches(';')
        .trim()
        .to_owned()
}

pub(crate) fn enum_header(declaration: &TSEnumDeclaration<'_>) -> String {
    let prefix = if declaration.r#const { "const " } else { "" };
    format!("{prefix}enum {}", declaration.id.name)
}

fn namespace_header(namespace: &TSNamespaceDeclaration<'_>) -> String {
    let mut name = namespace.id.name.to_string();
    let mut body = &namespace.body;
    while let oxc_ast::ast::TSNamespaceDeclarationBody::TSNamespaceDeclaration(inner) = body {
        name.push('.');
        name.push_str(inner.id.name.as_str());
        body = &inner.body;
    }
    format!("namespace {name}")
}

/// What the name declared at `span` in the file `path` is declared as.
pub(crate) fn declared_at(host: &dyn Host, path: &Path, span: (u32, u32)) -> Option<Declared> {
    let text = host.text(path)?;
    if span == (0, 0) {
        // A jump to a whole module (a namespace import, a module specifier).
        let stem = path.file_stem()?.to_string_lossy().into_owned();
        return Some(Declared {
            name: stem.clone(),
            kind: Kind::Module(stem),
            docs: None,
        });
    }
    with_semantic(path, &text, |_, semantic| {
        let nodes = semantic.nodes();
        let scoping = semantic.scoping();
        let binding = nodes.iter().find_map(|node| match node.kind() {
            AstKind::BindingIdentifier(id) if (id.span.start, id.span.end) == span => Some(id),
            _ => None,
        })?;
        let symbol = binding.symbol_id.get()?;
        let name = binding.name.to_string();
        let node = scoping.symbol_declaration(symbol);
        let docs = docs_before(semantic, &text, &doc_starts(semantic, node));
        let kind = match nodes.kind(node) {
            AstKind::Function(function) => {
                let (signature, returns) = function_signature(function, &text, &name);
                Kind::Function { signature, returns }
            }
            AstKind::Class(class) => Kind::Header(class_header(class, &text)),
            AstKind::TSInterfaceDeclaration(interface) => {
                Kind::Header(interface_header(interface, &text))
            }
            AstKind::TSTypeAliasDeclaration(alias) => Kind::Header(alias_header(alias, &text)),
            AstKind::TSEnumDeclaration(declaration) => Kind::Header(enum_header(declaration)),
            AstKind::TSNamespaceDeclaration(namespace) => Kind::Header(namespace_header(namespace)),
            AstKind::VariableDeclarator(declarator) => {
                let keyword = nodes
                    .ancestors(node)
                    .find_map(|ancestor| match ancestor.kind() {
                        AstKind::VariableDeclaration(declaration) => {
                            Some(declaration.kind.as_str().to_owned())
                        }
                        _ => None,
                    })
                    .unwrap_or_else(|| "var".to_owned());
                Kind::Variable {
                    keyword,
                    annotation: declarator
                        .type_annotation
                        .as_ref()
                        .map(|a| flat(&text, a.type_annotation.span())),
                }
            }
            AstKind::FormalParameter(parameter) => Kind::Parameter {
                annotation: parameter
                    .type_annotation
                    .as_ref()
                    .map(|a| flat(&text, a.type_annotation.span())),
            },
            AstKind::CatchParameter(_) | AstKind::BindingRestElement(_) => {
                Kind::Parameter { annotation: None }
            }
            // An import that leads nowhere, or anything else that binds a name: only the
            // name and its type.
            _ => Kind::Variable {
                keyword: "const".to_owned(),
                annotation: None,
            },
        };
        Some(Declared { name, kind, docs })
    })
    .flatten()
}

// ------------------------------------------------------------------------------------------
// What is under the pointer

/// A member written where it is declared: `get(): T` in a class, `x: number` in an
/// interface.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Member {
    name: String,
    /// The member as written, without its body.
    written: String,
    method: bool,
    annotated: bool,
    docs: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Subject {
    /// A name that is bound or used in the file.
    Name { span: (u32, u32) },
    /// The property in `object.property`: nothing here declares it.
    Access { span: (u32, u32), name: String },
    /// A member's name at its declaration.
    Declaration { span: (u32, u32), member: Member },
}

fn contains(span: Span, offset: u32) -> bool {
    span.start <= offset && offset <= span.end
}

fn modifiers(accessibility: Option<TSAccessibility>) -> &'static str {
    match accessibility {
        Some(TSAccessibility::Private) => "private ",
        Some(TSAccessibility::Protected) => "protected ",
        _ => "",
    }
}

fn subject_at(semantic: &Semantic<'_>, text: &str, offset: u32) -> Option<Subject> {
    let mut best: Option<(u32, Subject)> = None;
    let mut offer = |span: Span, subject: Subject| {
        let size = span.end - span.start;
        if best.as_ref().is_none_or(|(smallest, _)| size < *smallest) {
            best = Some((size, subject));
        }
    };
    for node in semantic.nodes().iter() {
        match node.kind() {
            AstKind::IdentifierReference(reference) if contains(reference.span, offset) => {
                offer(
                    reference.span,
                    Subject::Name {
                        span: (reference.span.start, reference.span.end),
                    },
                );
            }
            AstKind::BindingIdentifier(binding) if contains(binding.span, offset) => {
                offer(
                    binding.span,
                    Subject::Name {
                        span: (binding.span.start, binding.span.end),
                    },
                );
            }
            // An import or export name is followed by definition, like any other name.
            AstKind::ImportSpecifier(specifier) if contains(specifier.imported.span(), offset) => {
                let span = specifier.imported.span();
                offer(
                    span,
                    Subject::Name {
                        span: (span.start, span.end),
                    },
                );
            }
            AstKind::StaticMemberExpression(member) if contains(member.property.span, offset) => {
                offer(
                    member.property.span,
                    Subject::Access {
                        span: (member.property.span.start, member.property.span.end),
                        name: member.property.name.to_string(),
                    },
                );
            }
            AstKind::MethodDefinition(method) if contains(method.key.span(), offset) => {
                let key = method.key.span();
                let value = &method.value;
                let end = value
                    .return_type
                    .as_ref()
                    .map_or(value.params.span.end, |r| r.span.end);
                let written = format!(
                    "{}{}{}",
                    modifiers(method.accessibility),
                    if method.r#static { "static " } else { "" },
                    flat(text, Span::new(key.start, end))
                );
                offer(
                    key,
                    Subject::Declaration {
                        span: (key.start, key.end),
                        member: Member {
                            name: key_name(&method.key, text),
                            written,
                            method: true,
                            annotated: value.return_type.is_some(),
                            docs: docs_before(semantic, text, &doc_starts(semantic, node.id())),
                        },
                    },
                );
            }
            AstKind::PropertyDefinition(property) if contains(property.key.span(), offset) => {
                let key = property.key.span();
                let (end, annotated) = match &property.type_annotation {
                    Some(a) => (a.span.end, true),
                    None => (key.end, false),
                };
                let written = format!(
                    "{}{}{}",
                    modifiers(property.accessibility),
                    if property.r#static { "static " } else { "" },
                    flat(text, Span::new(key.start, end))
                );
                offer(
                    key,
                    Subject::Declaration {
                        span: (key.start, key.end),
                        member: Member {
                            name: key_name(&property.key, text),
                            written,
                            method: false,
                            annotated,
                            docs: docs_before(semantic, text, &doc_starts(semantic, node.id())),
                        },
                    },
                );
            }
            AstKind::TSPropertySignature(signature) if contains(signature.key.span(), offset) => {
                let key = signature.key.span();
                let (end, annotated) = match &signature.type_annotation {
                    Some(a) => (a.span.end, true),
                    None => (key.end, false),
                };
                offer(
                    key,
                    Subject::Declaration {
                        span: (key.start, key.end),
                        member: Member {
                            name: key_name(&signature.key, text),
                            written: flat(text, Span::new(key.start, end)),
                            method: false,
                            annotated,
                            docs: docs_before(semantic, text, &doc_starts(semantic, node.id())),
                        },
                    },
                );
            }
            AstKind::TSMethodSignature(signature) if contains(signature.key.span(), offset) => {
                let key = signature.key.span();
                let end = signature
                    .return_type
                    .as_ref()
                    .map_or(signature.params.span.end, |r| r.span.end);
                offer(
                    key,
                    Subject::Declaration {
                        span: (key.start, key.end),
                        member: Member {
                            name: key_name(&signature.key, text),
                            written: flat(text, Span::new(key.start, end)),
                            method: true,
                            annotated: signature.return_type.is_some(),
                            docs: docs_before(semantic, text, &doc_starts(semantic, node.id())),
                        },
                    },
                );
            }
            _ => {}
        }
    }
    best.map(|(_, subject)| subject)
}

// ------------------------------------------------------------------------------------------

fn block(code: &str, docs: Option<&str>) -> String {
    let mut out = format!("```ts\n{code}\n```");
    if let Some(docs) = docs {
        out.push_str("\n\n---\n\n");
        out.push_str(docs);
    }
    out
}

/// The header of a declared name, given the type the checker has for it (if any).
fn header(declared: &Declared, type_text: Option<&str>) -> String {
    let name = &declared.name;
    match &declared.kind {
        Kind::Function { signature, returns } => {
            if *returns {
                return signature.clone();
            }
            // No return type was written: the checker's reading of it completes the header.
            match type_text.and_then(split_signature) {
                Some((_, returned)) => format!("{signature}: {returned}"),
                None => signature.clone(),
            }
        }
        Kind::Variable {
            keyword,
            annotation,
        } => match (type_text, annotation) {
            (Some(text), _) => format!("{keyword} {name}: {text}"),
            (None, Some(annotation)) => format!("{keyword} {name}: {annotation}"),
            (None, None) => format!("{keyword} {name}"),
        },
        Kind::Parameter { annotation } => match (type_text, annotation) {
            (Some(text), _) => format!("(parameter) {name}: {text}"),
            (None, Some(annotation)) => format!("(parameter) {name}: {annotation}"),
            (None, None) => format!("(parameter) {name}"),
        },
        Kind::Header(header) => header.clone(),
        Kind::Module(module) => format!("module {module}"),
    }
}

/// What to show for the name at `offset` of `text` (the file at `path`). `type_at` is asked
/// for the checker's type at an offset of that same file.
pub fn hover(
    host: &dyn Host,
    path: &Path,
    text: &str,
    offset: usize,
    type_at: &dyn Fn(u32) -> Option<TypeAtOffset>,
) -> Option<Hover> {
    let offset32 = u32::try_from(offset).ok()?;
    let subject = with_semantic(path, text, |_, semantic| {
        subject_at(semantic, text, offset32)
    })
    .flatten()?;

    // The checker's type for the name, when it is about that name and not about something
    // that merely contains it.
    let type_for = |span: (u32, u32)| -> Option<String> {
        let found = type_at(offset32)?;
        (found.end == span.1 && found.start <= span.0).then_some(found.text)
    };

    match subject {
        Subject::Declaration { span, member } => {
            let type_text = type_for(span);
            let label = if member.method { "method" } else { "property" };
            let shown = if member.annotated {
                member.written.clone()
            } else if let Some(text) = type_text {
                match (member.method, signature_from_type(&member.name, &text)) {
                    (true, Some(signature)) => signature,
                    _ => format!("{}: {text}", member.written),
                }
            } else {
                member.written.clone()
            };
            Some(Hover {
                range: span,
                markdown: block(&format!("({label}) {shown}"), member.docs.as_deref()),
            })
        }
        Subject::Access { span, name } => {
            let text = type_for(span)?;
            let shown = match signature_from_type(&name, &text) {
                Some(signature) => format!("(method) {signature}"),
                None => format!("(property) {name}: {text}"),
            };
            Some(Hover {
                range: span,
                markdown: block(&shown, None),
            })
        }
        Subject::Name { span } => {
            let path_buf = path.to_path_buf();
            let hits = navigate::definition(host, &path_buf, text, offset);
            let type_text = type_for(span);
            let declared = hits
                .iter()
                .find_map(|hit| declared_at(host, &hit.path, hit.span));
            match declared {
                Some(declared) => Some(Hover {
                    range: span,
                    markdown: block(
                        &clip(header(&declared, type_text.as_deref())),
                        declared.docs.as_deref(),
                    ),
                }),
                // Nothing in the project declares it (a library global, say), but the checker
                // knows what it is.
                None => {
                    let name = text.get(span.0 as usize..span.1 as usize)?;
                    let type_text = type_text?;
                    let shown = match signature_from_type(name, &type_text) {
                        Some(signature) => format!("function {signature}"),
                        None => format!("const {name}: {type_text}"),
                    };
                    Some(Hover {
                        range: span,
                        markdown: block(&clip(shown), None),
                    })
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Arc;

    use super::*;

    struct Files(HashMap<PathBuf, Arc<str>>);

    impl Files {
        fn new(files: &[(&str, &str)]) -> Self {
            Self(
                files
                    .iter()
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

    /// The hover at the first `needle` (plus `into`) of `a.ts`, with the checker saying
    /// `types`: each is a needle, how many bytes of it the typed thing takes, and its type.
    fn show(
        files: &[(&str, &str)],
        needle: &str,
        into: usize,
        types: &[(&str, usize, &str)],
    ) -> Option<String> {
        let host = Files::new(files);
        let path = PathBuf::from("/p/a.ts");
        let text = host.text(&path).unwrap();
        let at = text.find(needle).expect("needle") + into;
        let types: Vec<(u32, u32, String)> = types
            .iter()
            .map(|(word, length, type_text)| {
                let start = text.find(word).expect("typed needle") as u32;
                (start, start + *length as u32, (*type_text).to_owned())
            })
            .collect();
        let type_at = move |offset: u32| {
            types
                .iter()
                .filter(|(start, end, _)| *start <= offset && offset <= *end)
                .min_by_key(|(start, end, _)| end - start)
                .map(|(start, end, text)| TypeAtOffset {
                    start: *start,
                    end: *end,
                    text: text.clone(),
                })
        };
        hover(&host, &path, &text, at, &type_at).map(|h| h.markdown)
    }

    fn code(markdown: &str) -> &str {
        markdown
            .strip_prefix("```ts\n")
            .and_then(|rest| rest.split("\n```").next())
            .unwrap_or(markdown)
    }

    #[test]
    fn a_signature_is_split_where_the_parameters_end() {
        assert_eq!(
            split_signature("(a: number) => string"),
            Some(("(a: number)", "string"))
        );
        assert_eq!(
            split_signature("<T>(x: T, f: (y: T) => void) => T[]"),
            Some(("<T>(x: T, f: (y: T) => void)", "T[]"))
        );
        assert_eq!(
            split_signature("(o: { a: () => void }) => void"),
            Some(("(o: { a: () => void })", "void"))
        );
        assert_eq!(split_signature("((a: number) => void) | undefined"), None);
        assert_eq!(split_signature("{ a: number }"), None);
        assert_eq!(split_signature("string"), None);
        assert_eq!(
            split_signature("() => () => number"),
            Some(("()", "() => number"))
        );
    }

    #[test]
    fn a_function_shows_its_signature_as_written() {
        let md = show(
            &[(
                "a.ts",
                "function twice(n: number): number { return n * 2; }\ntwice(2);\n",
            )],
            "twice(2)",
            0,
            &[],
        )
        .unwrap();
        assert_eq!(code(&md), "function twice(n: number): number");
    }

    #[test]
    fn a_function_without_a_return_type_takes_the_inferred_one() {
        let md = show(
            &[(
                "a.ts",
                "function add(a: number, b: number) { return a + b; }\nadd(1, 2);\n",
            )],
            "add(1",
            0,
            &[("add(1", 3, "(a: number, b: number) => number")],
        )
        .unwrap();
        assert_eq!(code(&md), "function add(a: number, b: number): number");
    }

    #[test]
    fn async_generics_and_generators_are_kept() {
        let md = show(
            &[(
                "a.ts",
                "async function load<T>(x: T): Promise<T> { return x; }\nload(1);\n",
            )],
            "load(1)",
            0,
            &[],
        )
        .unwrap();
        assert_eq!(code(&md), "async function load<T>(x: T): Promise<T>");
    }

    #[test]
    fn a_variable_shows_its_keyword_and_the_checkers_type() {
        let source = "const n = 1 + 2;\nlet s = 'x';\nvar w = [1];\nn; s; w;\n";
        let types = [("n;", 1, "number")];
        let md = show(&[("a.ts", source)], "n;", 0, &types).unwrap();
        assert_eq!(code(&md), "const n: number");
        let md = show(&[("a.ts", source)], "s;", 0, &[("s;", 1, "string")]).unwrap();
        assert_eq!(code(&md), "let s: string");
        let md = show(&[("a.ts", source)], "w;", 0, &[("w;", 1, "number[]")]).unwrap();
        assert_eq!(code(&md), "var w: number[]");
    }

    #[test]
    fn a_variable_with_no_checker_type_shows_its_annotation_or_just_its_name() {
        let md = show(
            &[("a.ts", "const a: Map<string, number> = m;\na;\n")],
            "a;",
            0,
            &[],
        )
        .unwrap();
        assert_eq!(code(&md), "const a: Map<string, number>");
        let md = show(&[("a.ts", "const b = m;\nb;\n")], "b;", 0, &[]).unwrap();
        assert_eq!(code(&md), "const b");
    }

    #[test]
    fn a_parameter_is_labelled() {
        let md = show(
            &[("a.ts", "function f(name: string) { return name; }\n")],
            "name:",
            0,
            &[("name:", 4, "string")],
        )
        .unwrap();
        assert_eq!(code(&md), "(parameter) name: string");
    }

    #[test]
    fn classes_interfaces_aliases_and_enums_show_their_declaration_line() {
        let source = "export class Box<T> extends Base implements Item { }\ninterface Shape<T = number> extends Other { n: T }\ntype Pair<A> = { a: A; b: A };\nconst enum Color { Red }\nenum Size { S }\nnamespace A.B { export const v = 1; }\nlet b: Box<number>; let s: Shape; let p: Pair<1>; Color; Size; A;\n";
        let t = |needle: &str| show(&[("a.ts", source)], needle, 0, &[]).unwrap();
        assert_eq!(
            code(&t("Box<number>")),
            "class Box<T> extends Base implements Item"
        );
        assert_eq!(
            code(&t("Shape;")),
            "interface Shape<T = number> extends Other"
        );
        assert_eq!(code(&t("Pair<1>")), "type Pair<A> = { a: A; b: A }");
        assert_eq!(code(&t("Color;")), "const enum Color");
        assert_eq!(code(&t("Size;")), "enum Size");
        assert_eq!(
            code(&show(&[("a.ts", source)], "Size; A;", 6, &[]).unwrap()),
            "namespace A.B"
        );
    }

    #[test]
    fn a_long_alias_is_cut() {
        let long = "x".repeat(600);
        let source = format!("type Long = '{long}';\nlet v: Long;\n");
        let md = show(&[("a.ts", &source)], "Long;", 0, &[]).unwrap();
        let line = code(&md);
        assert!(line.ends_with('…'));
        assert!(line.chars().count() <= MAX_HEADER + 1);
    }

    #[test]
    fn jsdoc_is_shown_under_the_header() {
        let source = "/**\n * Doubles a number.\n *\n * @param n - the number\n * @returns twice `n`\n * @deprecated use triple\n */\nexport function twice(n: number): number { return n * 2; }\ntwice(1);\n";
        let md = show(&[("a.ts", source)], "twice(1)", 0, &[]).unwrap();
        assert!(
            md.starts_with("```ts\nfunction twice(n: number): number\n```\n\n---\n\n"),
            "{md}"
        );
        assert!(md.contains("Doubles a number."));
        assert!(md.contains("*@param* `n` — the number"));
        assert!(md.contains("*@returns* twice `n`"));
        assert!(md.contains("**@deprecated** use triple"));
    }

    #[test]
    fn a_line_comment_or_a_plain_block_is_not_documentation() {
        let source = "// not docs\n/* nor this */\nfunction f(): void {}\nf();\n";
        let md = show(&[("a.ts", source)], "f();", 0, &[]).unwrap();
        assert!(!md.contains("---"), "{md}");
    }

    #[test]
    fn jsdoc_above_an_exported_const_is_found() {
        let source = "/** The answer. */\nexport const answer = 42;\nanswer;\n";
        let md = show(&[("a.ts", source)], "answer;", 0, &[("answer;", 6, "42")]).unwrap();
        assert!(md.contains("The answer."), "{md}");
        assert_eq!(code(&md), "const answer: 42");
    }

    #[test]
    fn the_documentation_of_an_imported_name_comes_from_where_it_is_declared() {
        let md = show(
            &[
                (
                    "lib.ts",
                    "/** Says hi. */\nexport function greet(who: string): void {}\n",
                ),
                ("a.ts", "import { greet } from './lib';\ngreet('x');\n"),
            ],
            "greet('x')",
            0,
            &[],
        )
        .unwrap();
        assert_eq!(code(&md), "function greet(who: string): void");
        assert!(md.contains("Says hi."));
    }

    #[test]
    fn jsdoc_formatting() {
        let md = jsdoc_markdown(
            "/**\n * Summary line\n * continues here.\n *\n * Second paragraph.\n * @param {string} name who to greet\n * @param count - how many\n * spanning lines\n * @example\n * greet('a');\n * @see other\n */",
        );
        assert_eq!(
            md,
            "Summary line\ncontinues here.\n\nSecond paragraph.\n\n*@param* `name` — who to greet\n\n*@param* `count` — how many spanning lines\n\n*@example*\n```ts\ngreet('a');\n```\n\n*@see* other"
        );
        assert_eq!(jsdoc_markdown("/** */"), "");
        assert_eq!(jsdoc_markdown("/** @deprecated */"), "**@deprecated**");
    }

    #[test]
    fn a_property_access_shows_the_type_of_the_access() {
        let source = "const o = { a: 1 };\no.a;\n";
        let md = show(&[("a.ts", source)], "a;", 0, &[("o.a", 3, "number")]).unwrap();
        assert_eq!(code(&md), "(property) a: number");
    }

    #[test]
    fn a_method_access_shows_a_signature() {
        let source = "const box = make();\nbox.get(1);\n";
        let md = show(
            &[("a.ts", source)],
            "get(1)",
            0,
            &[("box.get", 7, "(n: number) => string")],
        )
        .unwrap();
        assert_eq!(code(&md), "(method) get(n: number): string");
    }

    #[test]
    fn a_property_access_with_no_type_shows_nothing() {
        let md = show(&[("a.ts", "o.a;\n")], "a;", 0, &[]);
        assert_eq!(md, None);
    }

    #[test]
    fn members_at_their_declaration_show_as_written() {
        let source = "class Box<T> {\n  /** The item. */\n  item: T;\n  private count = 0;\n  get(): T { return this.item; }\n  static make(): Box<number> { return new Box(1); }\n}\ninterface I { size: number; grow(by: number): void }\n";
        let t = |needle: &str, types: &[(&str, usize, &str)]| {
            show(&[("a.ts", source)], needle, 0, types).unwrap()
        };
        let md = t("item: T", &[]);
        assert_eq!(code(&md), "(property) item: T");
        assert!(md.contains("The item."), "{md}");
        assert_eq!(code(&t("get()", &[])), "(method) get(): T");
        assert_eq!(
            code(&t("make()", &[])),
            "(method) static make(): Box<number>"
        );
        assert_eq!(code(&t("grow(", &[])), "(method) grow(by: number): void");
        assert_eq!(code(&t("size:", &[])), "(property) size: number");
        // No annotation: the checker's type fills in.
        assert_eq!(
            code(&t("count =", &[("count =", 5, "number")])),
            "(property) private count: number"
        );
    }

    #[test]
    fn a_name_nothing_declares_still_shows_the_checkers_type() {
        let md = show(
            &[("a.ts", "console.log(1);\n")],
            "console",
            0,
            &[("console", 7, "Console")],
        )
        .unwrap();
        assert_eq!(code(&md), "const console: Console");
    }

    #[test]
    fn nothing_is_shown_off_a_name() {
        let source = "const n = 1 + 2;\n";
        assert_eq!(
            show(&[("a.ts", source)], "+ 2", 0, &[]),
            None,
            "an operator"
        );
        assert_eq!(
            show(&[("a.ts", source)], "1 +", 0, &[("1 +", 1, "1")]),
            None,
            "a literal"
        );
        assert_eq!(
            show(&[("a.ts", source)], "const", 0, &[]),
            None,
            "a keyword"
        );
    }

    #[test]
    fn a_checker_type_about_something_larger_is_not_taken_for_the_name() {
        // The checker only knows the whole call; it says nothing of `f` itself.
        let md = show(
            &[("a.ts", "const f = make();\nf;\n")],
            "f;",
            0,
            &[("make()", 6, "string")],
        )
        .unwrap();
        assert_eq!(code(&md), "const f");
    }

    #[test]
    fn a_namespace_import_shows_the_module() {
        let md = show(
            &[
                ("lib.ts", "export const one = 1;\n"),
                ("a.ts", "import * as lib from './lib';\nlib;\n"),
            ],
            "lib;",
            0,
            &[],
        )
        .unwrap();
        assert_eq!(code(&md), "module lib");
    }

    #[test]
    fn the_range_is_the_name() {
        let host = Files::new(&[("a.ts", "const value = 1;\nvalue;\n")]);
        let path = PathBuf::from("/p/a.ts");
        let text = host.text(&path).unwrap();
        let at = text.rfind("value").unwrap() + 2;
        let found = hover(&host, &path, &text, at, &|_| None).unwrap();
        assert_eq!(
            found.range,
            (
                text.rfind("value").unwrap() as u32,
                text.rfind("value").unwrap() as u32 + 5
            )
        );
    }
}
