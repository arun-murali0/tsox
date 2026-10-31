//! `/// <reference path="..." />` and `/// <reference types="..." />` at the top of a file.
//!
//! Read by the module scan (each one is an edge, so the file it names is found) and by the
//! checker (an unresolved one is an error). `lib` and `no-default-lib` name nothing the
//! checker reads: its library is fixed, so they are accepted and ignored.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DirectiveKind {
    Path,
    Types,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Directive {
    pub kind: DirectiveKind,
    /// As an import would write it: a `path` is made relative.
    pub specifier: String,
    /// The value as the file wrote it.
    pub written: String,
    pub start: usize,
    pub end: usize,
}

/// The directives in the comment block at the top of the file (blank lines and other
/// comments may precede or sit between them; the first statement ends the block, as in
/// TypeScript).
pub(crate) fn scan(source: &str) -> Vec<Directive> {
    let mut found = Vec::new();
    let mut offset = 0usize;
    let mut in_block_comment = false;
    for line in source.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let trimmed = line.trim_start_matches(['\u{feff}', ' ', '\t', '\r', '\n']);
        if in_block_comment {
            if trimmed.contains("*/") {
                in_block_comment = false;
            }
            continue;
        }
        if trimmed.is_empty() {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("///") {
            let lead = line.len() - trimmed.len();
            if let Some((kind, written)) = attribute_of(rest) {
                let specifier = match kind {
                    DirectiveKind::Path
                        if !(written.starts_with('.') || written.starts_with('/')) =>
                    {
                        format!("./{written}")
                    }
                    _ => written.clone(),
                };
                found.push(Directive {
                    kind,
                    specifier,
                    written,
                    start: start + lead,
                    end: start + line.trim_end_matches(['\r', '\n']).len(),
                });
            }
            continue;
        }
        if trimmed.starts_with("//") {
            continue;
        }
        if trimmed.starts_with("/*") {
            in_block_comment = !trimmed.contains("*/");
            continue;
        }
        break;
    }
    found
}

fn attribute_of(rest: &str) -> Option<(DirectiveKind, String)> {
    let rest = rest.trim().strip_prefix("<reference")?;
    let attribute = |name: &str| -> Option<String> {
        let at = rest.find(&format!("{name}="))? + name.len() + 1;
        let quote = rest[at..]
            .chars()
            .next()
            .filter(|c| matches!(c, '"' | '\''))?;
        let value = &rest[at + 1..];
        Some(value[..value.find(quote)?].to_owned())
    };
    if let Some(path) = attribute("path") {
        return Some((DirectiveKind::Path, path));
    }
    attribute("types").map(|types| (DirectiveKind::Types, types))
}
