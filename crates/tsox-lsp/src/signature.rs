//! Signature help: the parameters of the call being typed, and the one the cursor is on.
//!
//! The call is found in the text before the cursor, with the scanner completion uses, so it
//! works while the call is still open (`foo(1, |`), which is when it is wanted and when no
//! parser can read the file. The signature itself comes from the checker's type of the callee
//! when it has one, and from the callee's declaration when it has not.

use std::path::Path;

use tsox::TypeAtOffset;

use crate::completion::{Context, Open, closed_prefix, scan};
use crate::hover::{self, Kind, signature_from_type, split_signature};
use crate::navigate::{self, Host};

/// What to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signature {
    pub label: String,
    /// Where each parameter is in the label, as UTF-16 offsets.
    pub parameters: Vec<(u32, u32)>,
    pub documentation: Option<String>,
    pub active_parameter: u32,
}

/// A call whose argument list is open at the cursor.
#[derive(Debug, PartialEq, Eq)]
struct Call {
    name: String,
    /// The end of the callee, and the start of the last character in it.
    callee_end: usize,
    probe: usize,
    dotted: bool,
    is_new: bool,
    commas: u32,
}

/// Words that put a `(` after them that is not a call.
const NOT_CALLS: &[&str] = &[
    "if",
    "for",
    "while",
    "switch",
    "catch",
    "function",
    "return",
    "typeof",
    "await",
    "yield",
    "void",
    "delete",
    "in",
    "of",
    "instanceof",
    "else",
    "do",
    "case",
    "with",
    "throw",
    "class",
    "interface",
    "type",
    "enum",
    "namespace",
    "get",
    "set",
    "async",
    "extends",
    "implements",
];

fn is_ident_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_' || ch == '$'
}

fn trailing_word(text: &str) -> &str {
    let start = text
        .char_indices()
        .rev()
        .take_while(|(_, ch)| is_ident_char(*ch))
        .last()
        .map_or(text.len(), |(index, _)| index);
    &text[start..]
}

fn call_at(prefix: &str, open: &Open) -> Option<Call> {
    let head = prefix[..open.at].trim_end();
    let name = trailing_word(head);
    if name.is_empty() {
        return None;
    }
    let before = head[..head.len() - name.len()].trim_end();
    let dotted = before.ends_with('.');
    if !dotted && NOT_CALLS.contains(&name) {
        return None;
    }
    let previous = trailing_word(before);
    // `function name(` and `class Name(` declare; they do not call.
    if !dotted && ["function", "class"].contains(&previous) {
        return None;
    }
    let probe = head.char_indices().last()?.0;
    Some(Call {
        name: name.to_owned(),
        callee_end: head.len(),
        probe,
        dotted,
        is_new: !dotted && previous == "new",
        commas: open.commas,
    })
}

/// Where each parameter of a signature label is, as UTF-16 offsets into the label. The list
/// is the first parenthesised group after the name and its generics; commas inside brackets
/// do not separate parameters.
pub fn parameter_ranges(label: &str) -> Vec<(u32, u32)> {
    let chars: Vec<(usize, char)> = label.char_indices().collect();
    let mut utf16 = Vec::with_capacity(chars.len() + 1);
    let mut at = 0u32;
    for (_, ch) in &chars {
        utf16.push(at);
        at += ch.len_utf16() as u32;
    }
    utf16.push(at);

    let arrow = |i: usize| i > 0 && chars[i - 1].1 == '=';
    let mut i = 0;
    while i < chars.len() && chars[i].1 != '(' && chars[i].1 != '<' {
        i += 1;
    }
    if i < chars.len() && chars[i].1 == '<' {
        let mut depth = 0;
        while i < chars.len() {
            match chars[i].1 {
                '<' => depth += 1,
                '>' if !arrow(i) => {
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
        while i < chars.len() && chars[i].1 != '(' {
            i += 1;
        }
    }
    if i >= chars.len() || chars[i].1 != '(' {
        return Vec::new();
    }
    i += 1;

    let mut out = Vec::new();
    let mut push = |from: usize, to: usize| {
        let mut from = from;
        let mut to = to;
        while from < to && chars[from].1.is_whitespace() {
            from += 1;
        }
        while to > from && chars[to - 1].1.is_whitespace() {
            to -= 1;
        }
        if from < to {
            out.push((utf16[from], utf16[to]));
        }
    };
    let mut depth = 0;
    let mut start = i;
    while i < chars.len() {
        match chars[i].1 {
            '(' | '[' | '{' | '<' => depth += 1,
            '>' if arrow(i) => {}
            ')' | ']' | '}' | '>' => {
                if depth == 0 {
                    push(start, i);
                    return out;
                }
                depth -= 1;
            }
            ',' if depth == 0 => {
                push(start, i);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    // An unterminated list: what was read.
    push(start, chars.len());
    out
}

/// The label of a function declared as `function name(...)`, without the keyword.
fn label_of_function(signature: &str) -> String {
    let mut label = signature;
    for prefix in ["async ", "function* ", "function "] {
        if let Some(rest) = label.strip_prefix(prefix) {
            label = rest;
        }
    }
    label.to_owned()
}

/// The signature of the call being typed at `offset` of `text` (the file at `path`), if
/// there is one. `type_at` gives the checker's type at an offset of that same file.
pub fn signature_help(
    host: &dyn Host,
    path: &Path,
    text: &str,
    offset: usize,
    type_at: &dyn Fn(u32) -> Option<TypeAtOffset>,
) -> Option<Signature> {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    let prefix = &text[..offset];
    let scanned = scan(prefix);
    if scanned.context != Context::Code {
        return None;
    }
    // The innermost parenthesis that opens a call; one that only groups is passed over.
    let call = scanned
        .open
        .iter()
        .rev()
        .filter(|open| open.ch == '(')
        .find_map(|open| call_at(prefix, open))?;

    // What the callee is declared as, for its documentation and when the checker has no type.
    let declared = {
        let mut hits = Vec::new();
        if !call.dotted {
            hits = navigate::definition(host, path, text, call.probe);
            if hits.is_empty() {
                hits =
                    navigate::definition(host, path, &closed_prefix(text, offset, "_"), call.probe);
            }
        }
        hits.iter()
            .find_map(|hit| hover::declared_at(host, &hit.path, hit.span))
    };

    let from_type = type_at(call.probe as u32)
        .filter(|found| found.end as usize == call.callee_end)
        .and_then(|found| {
            let shown = found.text.strip_prefix("new ").unwrap_or(&found.text);
            signature_from_type(&call.name, shown)
        });
    let from_declaration = declared.as_ref().and_then(|declared| match &declared.kind {
        Kind::Function { signature, .. } => Some(label_of_function(signature)),
        Kind::Variable {
            annotation: Some(annotation),
            ..
        } => signature_from_type(&call.name, annotation),
        _ => None,
    });
    let label = from_type.or(from_declaration)?;

    let parameters = parameter_ranges(&label);
    let active_parameter = match parameters.len() {
        0 => 0,
        length => {
            let last = parameters[length - 1];
            let is_rest = label
                .encode_utf16()
                .skip(last.0 as usize)
                .take(3)
                .collect::<Vec<_>>()
                == "...".encode_utf16().collect::<Vec<_>>();
            if call.commas as usize >= length && is_rest {
                (length - 1) as u32
            } else {
                call.commas
            }
        }
    };
    let _ = split_signature;
    Some(Signature {
        label,
        parameters,
        documentation: declared.and_then(|declared| declared.docs),
        active_parameter,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Arc;

    use super::*;

    struct Files(HashMap<PathBuf, Arc<str>>);

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

    /// The signature help at the `|` in `marked`; `types` are (needle, length, type text)
    /// the fake checker knows.
    fn help_with(
        extra: &[(&str, &str)],
        marked: &str,
        types: &[(&str, usize, &str)],
    ) -> Option<Signature> {
        let offset = marked.find('|').expect("a cursor");
        let text = marked.replacen('|', "", 1);
        let mut files: HashMap<PathBuf, Arc<str>> = extra
            .iter()
            .map(|(n, t)| (PathBuf::from(format!("/p/{n}")), Arc::from(*t)))
            .collect();
        files.insert(PathBuf::from("/p/a.ts"), Arc::from(text.as_str()));
        let host = Files(files);
        let types: Vec<(u32, u32, String)> = types
            .iter()
            .map(|(needle, length, ty)| {
                let start = text.find(needle).expect("typed needle") as u32;
                (start, start + *length as u32, (*ty).to_owned())
            })
            .collect();
        let type_at = move |at: u32| {
            types
                .iter()
                .filter(|(s, e, _)| *s <= at && at <= *e)
                .min_by_key(|(s, e, _)| e - s)
                .map(|(s, e, t)| TypeAtOffset {
                    start: *s,
                    end: *e,
                    text: t.clone(),
                })
        };
        signature_help(&host, Path::new("/p/a.ts"), &text, offset, &type_at)
    }

    fn help(marked: &str) -> Option<Signature> {
        help_with(&[], marked, &[])
    }

    fn shown(signature: &Signature) -> Vec<String> {
        let units: Vec<u16> = signature.label.encode_utf16().collect();
        signature
            .parameters
            .iter()
            .map(|(s, e)| String::from_utf16(&units[*s as usize..*e as usize]).unwrap())
            .collect()
    }

    #[test]
    fn a_declared_function_gives_its_label_and_the_parameter_under_the_cursor() {
        let source = "function add(a: number, b: number): number { return a + b; }\n";
        let s = help(&format!("{source}add(1, |)")).unwrap();
        assert_eq!(s.label, "add(a: number, b: number): number");
        assert_eq!(shown(&s), vec!["a: number", "b: number"]);
        assert_eq!(s.active_parameter, 1);
        assert_eq!(
            help(&format!("{source}add(|)")).unwrap().active_parameter,
            0
        );
        assert_eq!(
            help(&format!("{source}add(1|")).unwrap().active_parameter,
            0
        );
    }

    #[test]
    fn it_works_while_the_call_is_still_open() {
        let source = "function add(a: number, b: number): number { return a + b; }\n";
        let s = help(&format!("{source}function g() {{\n  add(1, |\n")).unwrap();
        assert_eq!(s.active_parameter, 1);
        assert_eq!(s.label, "add(a: number, b: number): number");
    }

    #[test]
    fn nested_calls_and_groupings_pick_the_right_call() {
        let source = "function add(a: number, b: number): number { return a + b; }\nfunction neg(x: number): number { return -x; }\n";
        let s = help(&format!("{source}add(neg(1), |)")).unwrap();
        assert_eq!(s.label, "add(a: number, b: number): number");
        assert_eq!(s.active_parameter, 1);
        let s = help(&format!("{source}add(neg(|))")).unwrap();
        assert_eq!(s.label, "neg(x: number): number");
        // A grouping paren is passed over.
        let s = help(&format!("{source}add((1 + 2), |)")).unwrap();
        assert_eq!(s.label, "add(a: number, b: number): number");
        let s = help(&format!("{source}add((1 + |")).unwrap();
        assert_eq!(s.label, "add(a: number, b: number): number");
        assert_eq!(s.active_parameter, 0);
    }

    #[test]
    fn commas_inside_brackets_do_not_count() {
        let source = "function f(a: number[], b: { x: number, y: number }, c: string) {}\n";
        assert_eq!(
            help(&format!("{source}f([1, 2, 3], |"))
                .unwrap()
                .active_parameter,
            1
        );
        assert_eq!(
            help(&format!("{source}f([1, 2], {{ x: 1, y: 2 }}, |"))
                .unwrap()
                .active_parameter,
            2
        );
        assert_eq!(
            help(&format!("{source}f([1, 2, |"))
                .unwrap()
                .active_parameter,
            0
        );
        let s = help(&format!("{source}f(|")).unwrap();
        assert_eq!(
            shown(&s),
            vec!["a: number[]", "b: { x: number, y: number }", "c: string"]
        );
    }

    #[test]
    fn generics_defaults_and_rest_parameters() {
        let source = "function id<T, U = number>(x: T, y: U, ...rest: string[]): T { return x; }\n";
        let s = help(&format!("{source}id(1, |")).unwrap();
        assert_eq!(shown(&s), vec!["x: T", "y: U", "...rest: string[]"]);
        assert_eq!(s.active_parameter, 1);
        // Past the end, a rest parameter keeps the highlight.
        assert_eq!(
            help(&format!("{source}id(1, 2, 'a', 'b', |"))
                .unwrap()
                .active_parameter,
            2
        );
    }

    #[test]
    fn a_checker_type_gives_the_signature_of_a_method() {
        let s = help_with(
            &[],
            "const box = make();\nbox.get(1, |",
            &[("box.get", 7, "(n: number, label: string) => string")],
        )
        .unwrap();
        assert_eq!(s.label, "get(n: number, label: string): string");
        assert_eq!(s.active_parameter, 1);
        assert_eq!(shown(&s), vec!["n: number", "label: string"]);
    }

    #[test]
    fn the_checkers_return_type_completes_a_function_that_did_not_write_one() {
        let source = "function add(a: number, b: number) { return a + b; }\n";
        let marked = format!("{source}add(|)");
        let s = help_with(
            &[],
            &marked,
            &[("\nadd(", 4, "(a: number, b: number) => number")],
        )
        .unwrap();
        assert_eq!(s.label, "add(a: number, b: number): number");
    }

    #[test]
    fn documentation_comes_from_the_declaration() {
        let s = help("/** Adds two numbers. */\nfunction add(a: number, b: number): number { return a + b; }\nadd(|)").unwrap();
        assert!(
            s.documentation
                .as_deref()
                .unwrap()
                .contains("Adds two numbers.")
        );
    }

    #[test]
    fn imported_functions_are_found_through_their_import() {
        let s = help_with(
            &[(
                "lib.ts",
                "export function greet(who: string, times: number): void {}\n",
            )],
            "import { greet } from './lib';\ngreet('a', |",
            &[],
        )
        .unwrap();
        assert_eq!(s.label, "greet(who: string, times: number): void");
        assert_eq!(s.active_parameter, 1);
    }

    #[test]
    fn variables_annotated_with_a_function_type_work() {
        let s = help("const cb: (a: number, b: string) => void = x;\ncb(1, |").unwrap();
        assert_eq!(s.label, "cb(a: number, b: string): void");
    }

    #[test]
    fn nothing_for_what_is_not_a_call() {
        assert_eq!(help("if (|"), None);
        assert_eq!(help("while (x) { for (|"), None);
        assert_eq!(help("function f(|"), None, "a declaration");
        assert_eq!(
            help("function add(a: number) {}\nadd('(|"),
            None,
            "in a string"
        );
        assert_eq!(
            help("function add(a: number) {}\nadd( // |"),
            None,
            "in a comment"
        );
        assert_eq!(help("const a = (|"), None, "a grouping");
        assert_eq!(help("unknownFn(|"), None, "no declaration and no type");
        assert_eq!(help("add(1)|"), None, "after the call is closed");
    }

    #[test]
    fn parameter_offsets_are_utf16() {
        let ranges = parameter_ranges("f(a: '\u{1F600}', b: number)");
        assert_eq!(ranges, vec![(2, 9), (11, 20)]);
        assert_eq!(parameter_ranges("f()"), Vec::<(u32, u32)>::new());
        assert_eq!(parameter_ranges("no parens"), Vec::<(u32, u32)>::new());
        assert_eq!(
            parameter_ranges("f(cb: (x: number) => void, n: number)"),
            vec![(2, 25), (27, 36)]
        );
    }
}
