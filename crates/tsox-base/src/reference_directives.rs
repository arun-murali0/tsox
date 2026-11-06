//! `/// <reference path="..." />`, `types`, and `lib` directives at the top of a file.
//!
//! `path` and `types` are ordinary type-only file edges. `lib` is resolved against the
//! vendored TypeScript standard-library declarations and becomes a library-reference edge.
//! `no-default-lib` remains metadata and does not name another file.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirectiveKind {
    Path,
    Types,
    Lib,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Directive {
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
pub fn scan(source: &str) -> Vec<Directive> {
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
        let needle = format!("{name}=");
        let (start, _) = rest.match_indices(&needle).find(|(at, _)| {
            *at == 0
                || rest[..*at]
                    .chars()
                    .next_back()
                    .is_some_and(char::is_whitespace)
        })?;
        let at = start + needle.len();
        let quote = rest[at..]
            .chars()
            .next()
            .filter(|c| matches!(c, '"' | '\''))?;
        let value = &rest[at + quote.len_utf8()..];
        Some(value[..value.find(quote)?].to_owned())
    };
    if let Some(path) = attribute("path") {
        return Some((DirectiveKind::Path, path));
    }
    if let Some(types) = attribute("types") {
        return Some((DirectiveKind::Types, types));
    }
    attribute("lib").map(|lib| (DirectiveKind::Lib, lib))
}

#[cfg(test)]
mod tests {
    use super::{DirectiveKind, scan};

    #[test]
    fn library_reference_is_distinct_from_no_default_lib() {
        let directives = scan(
            "/// <reference no-default-lib=\"true\" />\n\
             /// <reference lib=\"ES2020\" />\n",
        );
        assert_eq!(directives.len(), 1);
        assert_eq!(directives[0].kind, DirectiveKind::Lib);
        assert_eq!(directives[0].specifier, "ES2020");
    }

    #[test]
    fn path_and_types_directives_keep_their_existing_meaning() {
        let directives = scan(
            "/// <reference path=\"types.d.ts\" />\n\
             /// <reference types=\"node\" />\n",
        );
        assert_eq!(directives.len(), 2);
        assert_eq!(directives[0].kind, DirectiveKind::Path);
        assert_eq!(directives[0].specifier, "./types.d.ts");
        assert_eq!(directives[1].kind, DirectiveKind::Types);
        assert_eq!(directives[1].specifier, "node");
    }
}
