//! Reading a file that is being edited.
//!
//! The parser recovers from many errors, but a fatal one (an unclosed brace, which is what a
//! half-typed declaration is) leaves it with no tree at all. Every feature that reads syntax
//! goes through here, so that such a file still answers for what comes before the error: the
//! text is cut at the start of the line the error is on, and read again, until something
//! parses or nothing is left. The cut is a prefix, so every offset found is an offset into the
//! whole text.

use oxc_allocator::Allocator;
use oxc_ast::ast::Program;
use oxc_parser::Parser;
use oxc_span::SourceType;

/// How many times a file with a fatal error is cut back and read again.
const MAX_RETRIES: usize = 24;

/// Runs `read` on the syntax tree of `text`, read as the dialect `file_name`'s extension names
/// (TypeScript when it names none). `None` for a file nested too deeply to be parsed safely.
/// A file nothing could be read from gets an empty tree.
pub fn with_program<R>(
    text: &str,
    file_name: &str,
    read: impl FnOnce(&Program<'_>) -> R,
) -> Option<R> {
    if tsox_base::source_depth::max_bracket_depth(text) > tsox_base::stack_guard::MAX_BRACKET_DEPTH
    {
        return None;
    }
    let source_type = SourceType::from_path(file_name)
        .unwrap_or_else(|_| SourceType::default().with_typescript(true));
    let mut end = text.len();
    for _ in 0..MAX_RETRIES {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, &text[..end], source_type).parse();
        if !parsed.program.body.is_empty() || !parsed.diagnostics.has_errors() {
            return Some(read(&parsed.program));
        }
        let Some(at) = parsed
            .diagnostics
            .errors()
            .next()
            .and_then(|error| error.labels.first())
            .map(|label| label.offset() as usize)
        else {
            return Some(read(&parsed.program));
        };
        let cut = line_start_before(&text[..end], at.min(end));
        if cut >= end {
            return Some(read(&parsed.program));
        }
        end = cut;
    }
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, "", source_type).parse();
    Some(read(&parsed.program))
}

/// The start of the line holding the last non-blank character before `offset`. An error at
/// the very end of the text (an unclosed block) is on the last line that has text, not on
/// the empty one after it.
fn line_start_before(text: &str, offset: usize) -> usize {
    let mut upto = offset.min(text.len());
    while !text.is_char_boundary(upto) {
        upto -= 1;
    }
    let content = text[..upto].trim_end().len();
    text[..content].rfind('\n').map_or(0, |index| index + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count(text: &str, name: &str) -> Option<usize> {
        with_program(text, name, |program| program.body.len())
    }

    #[test]
    fn the_cut_point_is_the_start_of_the_line_of_the_error() {
        let text = "ab\ncd\nef\n";
        assert_eq!(line_start_before(text, 4), 3, "inside the second line");
        assert_eq!(
            line_start_before(text, text.len()),
            6,
            "at the end: the last line with text"
        );
        assert_eq!(line_start_before(text, 0), 0);
    }

    #[test]
    fn a_clean_file_is_read_whole() {
        assert_eq!(count("const a = 1;\nconst b = 2;\n", "a.ts"), Some(2));
    }

    #[test]
    fn a_fatal_error_keeps_what_came_before_it() {
        assert_eq!(
            count("const a = 1;\nclass Half {\n  m() {\n", "a.ts"),
            Some(1)
        );
    }

    #[test]
    fn a_file_that_is_only_an_error_is_empty() {
        assert_eq!(count("}}}", "a.ts"), Some(0));
    }

    #[test]
    fn a_file_nested_past_the_limit_is_refused() {
        let text = "(".repeat(tsox_base::stack_guard::MAX_BRACKET_DEPTH + 10);
        assert_eq!(count(&text, "a.ts"), None);
    }
}
