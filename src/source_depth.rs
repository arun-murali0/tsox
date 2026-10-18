// How deeply a source file nests its brackets, found without recursing.
//
// Problem: the parser recurses once per level of nesting, and so does the semantic builder
// after it. A stack overflow is an abort, and nothing in the checker can stop the parser
// from walking into one, so a file nested far enough took the whole run down.
// Tried: the depth guard in the checker's own walks, which cannot reach the parser's; and a
// bigger stack, which moves the limit and does not remove it.
// Picked: count the nesting of `(`, `[` and `{` in one pass over the bytes, before the
// parser sees the file, and refuse a file past a limit with an error that says so. The
// count skips comments, strings and template literals, so brackets inside them do not add
// to it, and it uses a counter and not recursion, so it is safe at any depth.
// Cost: it is a count and not a parse. A regular expression literal that contains a quote
// or a backtick can confuse it for the rest of the line, or until the next backtick, which
// can hide some nesting from the count or add some to it. The limit is far above what
// real code reaches, so that matters only for a file built to be deep.
// Now: used before every parse, in the checker and in the walk that finds a file's imports,
// with the limit in `stack_guard::MAX_BRACKET_DEPTH`. Bracketed nesting is refused at any
// depth, measured at 20,000, 100,000 and 200,000. Three shapes with no brackets to count can
// still abort a debug build at default stacks: a flat chain (`a + b + ...`, `o.a.a.a`,
// `c ? 1 : c ? 2 : ...`, `() => () => ...`) at 200,000 terms, a call chain (`o().a().a()`) at
// 100,000, and nested generic arguments (`Array<Array<...>>`) at 100,000. A `<` cannot be
// counted without a parse, since it is also less-than. None of them is code anyone writes.

/// The deepest nesting of `(`, `[` and `{` in `source`, ignoring brackets in comments,
/// strings and template literals. A closing bracket with nothing open does not go below
/// zero, so a stray one does not hide later nesting.
pub(crate) fn max_bracket_depth(source: &str) -> usize {
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut deepest = 0usize;
    // The depth at which each `${` inside a template literal was opened, so the `}` that
    // closes it returns to the template instead of to code.
    let mut interpolations: Vec<usize> = Vec::new();
    let mut in_template = false;
    let mut index = 0usize;

    while index < bytes.len() {
        let byte = bytes[index];

        if in_template {
            match byte {
                b'\\' => index += 1,
                b'`' => in_template = false,
                b'$' if bytes.get(index + 1) == Some(&b'{') => {
                    depth += 1;
                    deepest = deepest.max(depth);
                    interpolations.push(depth);
                    in_template = false;
                    index += 1;
                }
                _ => {}
            }
            index += 1;
            continue;
        }

        match byte {
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                while index < bytes.len() && bytes[index] != b'\n' {
                    index += 1;
                }
                continue;
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index += 2;
                while index < bytes.len()
                    && !(bytes[index] == b'*' && bytes.get(index + 1) == Some(&b'/'))
                {
                    index += 1;
                }
                index += 2;
                continue;
            }
            b'\'' | b'"' => {
                // A string ends at its quote or at the end of the line.
                index += 1;
                while index < bytes.len() && bytes[index] != byte && bytes[index] != b'\n' {
                    if bytes[index] == b'\\' {
                        index += 1;
                    }
                    index += 1;
                }
            }
            b'`' => in_template = true,
            b'(' | b'[' | b'{' => {
                depth += 1;
                deepest = deepest.max(depth);
            }
            b')' | b']' => depth = depth.saturating_sub(1),
            b'}' => {
                if interpolations.last() == Some(&depth) {
                    interpolations.pop();
                    in_template = true;
                }
                depth = depth.saturating_sub(1);
            }
            _ => {}
        }
        index += 1;
    }
    deepest
}

#[cfg(test)]
mod tests {
    use super::max_bracket_depth;

    #[test]
    fn plain_nesting_is_counted() {
        assert_eq!(max_bracket_depth(""), 0);
        assert_eq!(max_bracket_depth("const a = 1;"), 0);
        assert_eq!(max_bracket_depth("f(g([{}]))"), 4);
        assert_eq!(max_bracket_depth("{ { } } { }"), 2);
    }

    #[test]
    fn a_stray_closer_does_not_hide_later_nesting() {
        assert_eq!(max_bracket_depth(")))) ((("), 3);
    }

    #[test]
    fn brackets_in_comments_and_strings_are_ignored() {
        assert_eq!(max_bracket_depth("// (((((\nf()"), 1);
        assert_eq!(max_bracket_depth("/* [[[[ */ f()"), 1);
        assert_eq!(max_bracket_depth("const s = '((((' + \"[[[[\";"), 0);
        assert_eq!(max_bracket_depth("const s = 'it\\'s (((';"), 0);
    }

    #[test]
    fn an_unterminated_string_ends_at_the_line() {
        assert_eq!(max_bracket_depth("const s = 'oops\n((("), 3);
    }

    #[test]
    fn a_template_literal_hides_its_text_but_not_its_interpolations() {
        assert_eq!(max_bracket_depth("`((((`"), 0);
        assert_eq!(max_bracket_depth("`a ${ f(g(1)) } b ((((`"), 3);
        // The interpolation returns to the template, so the brackets after it are text.
        assert_eq!(max_bracket_depth("`${x}(((` + f()"), 1);
        assert_eq!(max_bracket_depth("`${ `${ (1) }` }`"), 3);
    }

    #[test]
    fn a_very_deep_file_is_counted_without_recursion() {
        let source = format!("{}1{}", "(".repeat(2_000_000), ")".repeat(2_000_000));
        assert_eq!(max_bracket_depth(&source), 2_000_000);
    }
}
