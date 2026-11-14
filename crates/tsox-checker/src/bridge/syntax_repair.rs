// Source repairs for constructs the parser gives up on and tsc recovers from.
//
// When the parser stops at a construct tsc reports as one syntax error and then keeps going, the
// source is patched into a form the parser accepts, parsed again, and tsc's error for the original
// is recorded. The patched copy is longer or shorter than the original, so every offset reported
// out of the check goes back through an `OffsetMap`. Each rule names the one construct it repairs
// and what tsc makes of it; a rule that fires on anything else would hide real errors.

use oxc_span::Span;

use crate::diagnostic_codes::DiagnosticCode as Code;
use crate::diagnostic_messages::messages;

use super::syntax_errors::SyntaxDiagnostic;

pub const MAX_REPAIRS: usize = 24;

#[derive(Clone, Copy, Debug)]
struct Segment {
    cur_start: usize,
    cur_end: usize,
    orig_start: usize,
    orig_end: usize,
    replaced: bool,
    // A stretch of the replacement that copies the original verbatim: (offset in the replacement,
    // offset in the original from the segment's start, length). Offsets inside it map exactly.
    copy: Option<(usize, usize, usize)>,
}

/// Offsets of a patched copy back to the original source.
#[derive(Clone, Debug)]
pub struct OffsetMap {
    segments: Vec<Segment>,
}

impl OffsetMap {
    pub fn to_original(&self, offset: u32, is_end: bool) -> u32 {
        let offset = offset as usize;
        for segment in &self.segments {
            if offset < segment.cur_end || (offset == segment.cur_end && !segment.replaced) {
                if let Some((repl, orig, len)) = segment.copy {
                    let inner = offset.wrapping_sub(segment.cur_start + repl);
                    if offset >= segment.cur_start + repl && inner <= len {
                        return (segment.orig_start + orig + inner) as u32;
                    }
                }
                if segment.replaced {
                    return if is_end {
                        segment.orig_end
                    } else {
                        segment.orig_start
                    } as u32;
                }
                return (segment.orig_start + offset - segment.cur_start) as u32;
            }
        }
        self.segments.last().map_or(offset, |last| last.orig_end) as u32
    }
}

struct Edit {
    start: usize,
    end: usize,
    with: String,
    copy: Option<(usize, usize, usize)>,
}

struct Note {
    message: crate::diagnostic_messages::DiagnosticMessage,
    start: usize,
    end: usize,
}

pub struct Repairs<'s> {
    original: &'s str,
    edits: Vec<Edit>,
    notes: Vec<Note>,
    text: String,
    segments: Vec<Segment>,
}

struct Rule {
    start: usize,
    end: usize,
    with: String,
    copy: Option<(usize, usize, usize)>,
    note: Note,
}

impl<'s> Repairs<'s> {
    pub fn new(original: &'s str) -> Self {
        let mut repairs = Self {
            original,
            edits: Vec::new(),
            notes: Vec::new(),
            text: String::new(),
            segments: Vec::new(),
        };
        repairs.rebuild();
        repairs
    }

    pub fn len(&self) -> usize {
        self.edits.len()
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    fn rebuild(&mut self) {
        self.edits.sort_by_key(|edit| edit.start);
        self.text.clear();
        self.segments.clear();
        let mut at = 0;
        for edit in &self.edits {
            if edit.start > at {
                self.segments.push(Segment {
                    cur_start: self.text.len(),
                    cur_end: self.text.len() + (edit.start - at),
                    orig_start: at,
                    orig_end: edit.start,
                    replaced: false,
                    copy: None,
                });
                self.text.push_str(&self.original[at..edit.start]);
            }
            self.segments.push(Segment {
                cur_start: self.text.len(),
                cur_end: self.text.len() + edit.with.len(),
                orig_start: edit.start,
                orig_end: edit.end,
                replaced: true,
                copy: edit.copy,
            });
            self.text.push_str(&edit.with);
            at = edit.end;
        }
        if at < self.original.len() || self.segments.is_empty() {
            self.segments.push(Segment {
                cur_start: self.text.len(),
                cur_end: self.text.len() + (self.original.len() - at),
                orig_start: at,
                orig_end: self.original.len(),
                replaced: false,
                copy: None,
            });
            self.text.push_str(&self.original[at..]);
        }
    }

    fn map(&self) -> OffsetMap {
        OffsetMap {
            segments: self.segments.clone(),
        }
    }

    // Repairs the construct the parser stopped at, if a rule knows it. Offsets are the patched
    // text's, and a stop inside text an earlier repair wrote is never repaired again.
    pub fn apply(&mut self, start: usize, end: usize, message: &str) -> bool {
        let Some(rule) = rule_for(&self.text, start, end, message) else {
            return false;
        };
        let map = self.map();
        let touches_patch = self.segments.iter().any(|segment| {
            segment.replaced && rule.start < segment.cur_end && rule.end > segment.cur_start
        });
        if touches_patch {
            return false;
        }
        let (orig_start, orig_end) = (
            map.to_original(rule.start as u32, false) as usize,
            map.to_original(rule.end as u32, true) as usize,
        );
        let note = Note {
            message: rule.note.message,
            start: map.to_original(rule.note.start as u32, false) as usize,
            end: map.to_original(rule.note.end as u32, true) as usize,
        };
        self.edits.push(Edit {
            start: orig_start,
            end: orig_end,
            with: rule.with,
            copy: rule.copy,
        });
        self.notes.push(note);
        self.rebuild();
        true
    }

    // The syntax errors of the final parse and the ones the repairs recorded, all at original
    // offsets, and the map the rest of the check reports through.
    pub fn finish(
        self,
        syntax: Vec<SyntaxDiagnostic>,
    ) -> (Vec<SyntaxDiagnostic>, Option<OffsetMap>) {
        if self.edits.is_empty() {
            return (syntax, None);
        }
        let map = self.map();
        let mut all: Vec<SyntaxDiagnostic> = syntax
            .into_iter()
            .map(|found| SyntaxDiagnostic {
                span: Span::new(
                    map.to_original(found.span.start, false),
                    map.to_original(found.span.end, true),
                ),
                message: found.message,
            })
            .collect();
        all.extend(self.notes.into_iter().map(|note| SyntaxDiagnostic {
            span: Span::new(note.start as u32, note.end as u32),
            message: note.message,
        }));
        (all, Some(map))
    }
}

fn is_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$' || byte == b'.'
}

fn rule_for(text: &str, start: usize, end: usize, message: &str) -> Option<Rule> {
    match message {
        "Unexpected token" if text.get(start..end) == Some("]") => {
            element_without_argument(text, start, end)
        }
        "An index signature must have a type annotation." => {
            index_signature_without_type(text, start, end)
        }
        "Expected `,` or `}` but found `?`" if text.get(start..end) == Some("?") => member_marker(
            start,
            end,
            Code::ObjectMemberOptional,
            "An object member cannot be declared optional.",
        ),
        "Expected `,` or `}` but found `!`" if text.get(start..end) == Some("!") => member_marker(
            start,
            end,
            Code::DefiniteAssignmentNotPermitted,
            "A definite assignment assertion '!' is not permitted in this context.",
        ),
        "Expected `,` or `}` but found `:`" | "Expected `,` or `}` but found `;`"
            if matches!(text.get(start..end), Some(":" | ";"))
                && super::syntax_errors::inside_enum(text, start) =>
        {
            enum_member_separator(start, end)
        }
        "Expected `)` but found `=`" => catch_initializer(text, start),
        "Expected `,` or `]` but found `=`" => index_signature_initializer(text, start),
        "A rest parameter must be last in a parameter list" => rest_parameter_not_last(text, start),
        "Cannot assign to this expression" => assignment_target(text, start, end),
        "An enum member cannot have a numeric name." => enum_numeric_name(text, start, end),
        "Computed property names are not allowed in enums." => enum_computed_name(text, start, end),
        _ => None,
    }
}

// `a[]` and `new A[]`: tsc reports TS1011 and treats the access as an error-typed value, so the
// access becomes `(a as any)`, which names the same thing and has the same type.
fn element_without_argument(text: &str, start: usize, end: usize) -> Option<Rule> {
    let bytes = text.as_bytes();
    let mut at = start;
    while at > 0 && bytes[at - 1].is_ascii_whitespace() {
        at -= 1;
    }
    if at == 0 || bytes[at - 1] != b'[' {
        return None;
    }
    at -= 1;
    // The expression and its `[` are on one line: `a: B` then `[]` on the next line is a type
    // followed by an index signature, which tsc reports as TS1096.
    while at > 0 && bytes[at - 1].is_ascii_whitespace() {
        if bytes[at - 1] == b'\n' {
            return None;
        }
        at -= 1;
    }
    let chain_end = at;
    while at > 0 && is_name_byte(bytes[at - 1]) {
        at -= 1;
    }
    let chain = text.get(at..chain_end)?;
    let mut annotated = at;
    while annotated > 0 && matches!(bytes[annotated - 1], b' ' | b'\t') {
        annotated -= 1;
    }
    if annotated > 0 && bytes[annotated - 1] == b':' {
        return None;
    }
    if chain.is_empty()
        || chain.starts_with(|c: char| c.is_ascii_digit() || c == '.')
        || chain.ends_with('.')
    {
        return None;
    }
    let mut from = at;
    let mut before = at;
    while before > 0 && bytes[before - 1].is_ascii_whitespace() {
        before -= 1;
    }
    if before >= 3
        && &text[before - 3..before] == "new"
        && (before == 3 || !is_name_byte(bytes[before - 4]))
    {
        from = before - 3;
    }
    Some(Rule {
        start: from,
        end,
        with: format!("({chain} as any)"),
        copy: Some((1, at - from, chain.len())),
        note: Note {
            message: messages::syntax(
                Code::ElementAccessNeedsArgument,
                "An element access expression should take an argument.".to_string(),
            ),
            start,
            end,
        },
    })
}

// `[key: string];` with no type: tsc reports TS1021 on the signature and gives it type `any`. tsc
// checks the key first, so an optional key (TS1019) or a key type other than string, number or
// symbol (TS1268) is not this error and is left alone.
fn index_signature_without_type(text: &str, start: usize, end: usize) -> Option<Rule> {
    let signature = text.get(start..end)?;
    if !(signature.starts_with('[') && signature.ends_with(']')) || !plain_index_key(signature) {
        return None;
    }
    Some(Rule {
        start: end,
        end,
        with: ": any".to_string(),
        copy: None,
        note: Note {
            message: messages::syntax(
                Code::IndexSignatureNeedsType,
                "An index signature must have a type annotation.".to_string(),
            ),
            start,
            end,
        },
    })
}

// Whether `[name: string | number | symbol]` is a plain key (no `?`, an allowed key type).
fn plain_index_key(signature: &str) -> bool {
    let inner = &signature[1..signature.len() - 1];
    let Some((name, key_type)) = inner.split_once(':') else {
        return false;
    };
    !name.contains('?') && matches!(key_type.trim(), "string" | "number" | "symbol")
}

// `enum E { 1 = 0 }`: tsc reports TS2452 on the name and keeps the member under that name, so
// the name becomes the same name as a string literal.
fn enum_numeric_name(text: &str, start: usize, end: usize) -> Option<Rule> {
    let name = text.get(start..end)?;
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(Rule {
        start,
        end,
        with: format!("\"{name}\""),
        copy: None,
        note: Note {
            message: messages::syntax(
                Code::EnumMemberNumericName,
                "An enum member cannot have a numeric name.".to_string(),
            ),
            start,
            end,
        },
    })
}

// `enum E { [k] = 0 }`: tsc reports TS1164 on the computed name. The member cannot be reached by
// any written name, so it becomes one with a name nothing refers to, unique to this position.
fn enum_computed_name(text: &str, start: usize, end: usize) -> Option<Rule> {
    let bytes = text.as_bytes();
    let mut from = start;
    while from > 0 && bytes[from - 1].is_ascii_whitespace() {
        from -= 1;
    }
    if from == 0 || bytes[from - 1] != b'[' {
        return None;
    }
    from -= 1;
    let mut to = end;
    while to < bytes.len() && bytes[to].is_ascii_whitespace() {
        to += 1;
    }
    if to >= bytes.len() || bytes[to] != b']' {
        return None;
    }
    to += 1;
    Some(Rule {
        start: from,
        end: to,
        with: format!("\"__computed_{from}\""),
        copy: None,
        note: Note {
            message: messages::syntax(
                Code::EnumComputedName,
                "Computed property names are not allowed in enums.".to_string(),
            ),
            start: from,
            end: to,
        },
    })
}

// `function f(...a, b) {}`: tsc reports TS1014 on the rest parameter and, because only the last
// parameter is a rest parameter to it, treats `a` as an ordinary parameter of type `any[]`. The
// dots go and an unannotated name is given that type, so the body sees the same type.
fn rest_parameter_not_last(text: &str, start: usize) -> Option<Rule> {
    let rest = text.get(start..)?.strip_prefix("...")?;
    let name_len = rest
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'$')
        .count();
    if name_len == 0 || rest.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let after = rest[name_len..].trim_start();
    let name_end = start + 3 + name_len;
    let (end, with) = if after.starts_with(':') {
        (start + 3, String::new())
    } else if after.starts_with(',') || after.starts_with(')') {
        (name_end, format!("{}: any[]", &rest[..name_len]))
    } else {
        return None;
    };
    Some(Rule {
        start,
        end,
        with,
        copy: None,
        note: Note {
            message: messages::syntax(
                Code::RestParameterNotLast,
                "A rest parameter must be last in a parameter list.".to_string(),
            ),
            start,
            end: name_end,
        },
    })
}

// `1 = 2;`, `f() = x;`, `(1, x) = 0;`, `1 >>= 2;`: tsc reports TS2364 on the left side (with its
// parentheses) and still checks it, so the left side becomes `(left as any).__tsox_repair`: a
// property of an `any` value is a target oxc accepts and the checker leaves alone, and `left`
// itself is still checked as an expression. `++`/`--` operands and `for (x in y)` heads have other
// codes (TS2357, TS2356, TS2405, TS2406) that depend on the operand's type, so they are left alone.
fn assignment_target(text: &str, start: usize, end: usize) -> Option<Rule> {
    text.get(start..end)?;
    let (mut start, mut end) = (start, end);
    loop {
        let before = text.get(..start)?.trim_end();
        let after = text.get(end..)?;
        let trimmed = after.trim_start();
        if before.ends_with('(') && trimmed.starts_with(')') {
            start = before.len() - 1;
            end = end + (after.len() - trimmed.len()) + 1;
        } else {
            break;
        }
    }
    let left = text.get(start..end)?;
    if left.is_empty() || left.starts_with("++") || left.starts_with("--") {
        return None;
    }
    let before = text.get(..start)?.trim_end();
    if before.ends_with("++") || before.ends_with("--") {
        return None;
    }
    let after = text.get(end..)?.trim_start();
    const OPERATORS: [&str; 16] = [
        ">>>=", "**=", "<<=", ">>=", "&&=", "||=", "??=", "+=", "-=", "*=", "/=", "%=", "&=", "|=",
        "^=", "=",
    ];
    let operator = OPERATORS.iter().find(|op| after.starts_with(**op))?;
    if *operator == "=" && (after.starts_with("==") || after.starts_with("=>")) {
        return None;
    }
    // `await using [a] = x` is a declaration, not an assignment.
    if before.ends_with("using") {
        return None;
    }
    // tsc checks an arithmetic compound assignment's left operand before its target: `this *= 1`
    // is TS2362, not TS2364. Only a number literal is known to pass that check from the text.
    let number = left.trim_matches(|c| c == '(' || c == ')' || c == ' ');
    if *operator != "="
        && !(!number.is_empty() && number.bytes().all(|b| b.is_ascii_digit() || b == b'.'))
    {
        return None;
    }
    Some(Rule {
        start,
        end,
        with: format!("({left} as any).__tsox_repair"),
        copy: Some((1, 0, left.len())),
        note: Note {
            message: messages::syntax(
                Code::AssignmentTargetInvalid,
                "The left-hand side of an assignment expression must be a variable or a property access."
                    .to_string(),
            ),
            start,
            end,
        },
    })
}

// `{ a?: 1 }`, `{ f?() {} }`, `{ a? }`, `{ a! }`: tsc reports the marker (TS1162, TS1255) and keeps
// the member without it, so the marker goes.
fn member_marker(start: usize, end: usize, code: Code, text: &str) -> Option<Rule> {
    Some(Rule {
        start,
        end,
        with: String::new(),
        copy: None,
        note: Note {
            message: messages::syntax(code, text.to_string()),
            start,
            end,
        },
    })
}

// `enum E { a: 1 }`: tsc reports TS1357 on the `:` and goes on as if it were a `,`, so `1` is
// the next member (and TS2452 reports its numeric name).
fn enum_member_separator(start: usize, end: usize) -> Option<Rule> {
    Some(Rule {
        start,
        end,
        with: ",".to_string(),
        copy: None,
        note: Note {
            message: messages::syntax(
                Code::EnumMemberSeparator,
                "An enum member name must be followed by a ',', '=', or '}'.".to_string(),
            ),
            start,
            end,
        },
    })
}

fn is_ident_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$'
}

// `catch (e = 1) {}`: tsc reports TS1197 on the initializer and drops it.
fn catch_initializer(text: &str, start: usize) -> Option<Rule> {
    if text.get(start..start + 1)? != "=" {
        return None;
    }
    let before = text.get(..start)?.trim_end();
    let name_start = before
        .bytes()
        .rposition(|b| !is_ident_byte(b))
        .map_or(0, |i| i + 1);
    if name_start == before.len() {
        return None;
    }
    let head = before[..name_start].trim_end().strip_suffix('(')?;
    head.trim_end().strip_suffix("catch")?;
    let mut depth = 0usize;
    let mut close = None;
    for (offset, byte) in text.bytes().enumerate().skip(start + 1) {
        match byte {
            b'(' => depth += 1,
            b')' if depth == 0 => {
                close = Some(offset);
                break;
            }
            b')' => depth -= 1,
            _ => {}
        }
    }
    let close = close?;
    let initializer = &text[start + 1..close];
    let lead = initializer.len() - initializer.trim_start().len();
    let (from, to) = (
        start + 1 + lead,
        start + 1 + lead + initializer.trim().len(),
    );
    if from == to {
        return None;
    }
    Some(Rule {
        start,
        end: close,
        with: String::new(),
        copy: None,
        note: Note {
            message: messages::syntax(
                Code::CatchVariableInitializer,
                "Catch clause variable cannot have an initializer.".to_string(),
            ),
            start: from,
            end: to,
        },
    })
}

// `[a: number = 1]: number`: tsc reports TS1020 on the parameter name and drops the initializer.
fn index_signature_initializer(text: &str, start: usize) -> Option<Rule> {
    if text.get(start..start + 1)? != "=" {
        return None;
    }
    let before = text.get(..start)?;
    let open = before.rfind('[')?;
    let inside = &before[open + 1..];
    let (name, key_type) = inside.split_once(':')?;
    if inside.contains(']') || !matches!(key_type.trim(), "string" | "number" | "symbol") {
        return None;
    }
    let name_text = name.trim();
    if name_text.is_empty() || !name_text.bytes().all(is_ident_byte) {
        return None;
    }
    let close = start + text[start..].find(']')?;
    let name_start = open + 1 + (name.len() - name.trim_start().len());
    Some(Rule {
        start: before.trim_end().len(),
        end: close,
        with: String::new(),
        copy: None,
        note: Note {
            message: messages::syntax(
                Code::IndexSignatureParameterInitializer,
                "An index signature parameter cannot have an initializer.".to_string(),
            ),
            start: name_start,
            end: name_start + name_text.len(),
        },
    })
}
