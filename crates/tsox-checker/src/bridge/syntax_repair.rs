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

// One way to repair a syntax error so the rest of the file can still be checked. A repair names
// the parser message it answers (`applies`) and what it does (`run`). The first repair that
// applies decides, even when its `run` answers `None`: a later row never sees a message an
// earlier row claimed, which is how the match this table replaced behaved.
struct Repair {
    #[cfg_attr(not(test), allow(dead_code))]
    name: &'static str,
    #[cfg_attr(not(test), allow(dead_code))]
    doc: &'static str,
    applies: fn(text: &str, start: usize, end: usize, message: &str) -> bool,
    run: fn(text: &str, start: usize, end: usize) -> Option<Rule>,
}

const REPAIRS: &[Repair] = &[
    // A class member modifier where a statement belongs is TS1128, which no semicolon is supplied
    // for, so this is tried before any rule for the same message.
    Repair {
        name: "member_modifier_statement",
        doc: "A class member modifier where a statement belongs.",
        applies: |_, _, _, m| m.starts_with("Expected a semicolon or an implicit semicolon"),
        run: |t, s, _| member_modifier_statement(t, s),
    },
    Repair {
        name: "element_without_argument",
        doc: "`a[]` and `new A[]`.",
        applies: |t, s, e, m| m == "Unexpected token" && t.get(s..e) == Some("]"),
        run: element_without_argument,
    },
    Repair {
        name: "index_signature_without_type",
        doc: "An index signature with no type annotation.",
        applies: |_, _, _, m| m == "An index signature must have a type annotation.",
        run: index_signature_without_type,
    },
    Repair {
        name: "optional_object_member",
        doc: "`{ a?: 1 }`: an object member declared optional.",
        applies: |t, s, e, m| m == "Expected `,` or `}` but found `?`" && t.get(s..e) == Some("?"),
        run: |_, s, e| {
            member_marker(
                s,
                e,
                Code::ObjectMemberOptional,
                "An object member cannot be declared optional.",
            )
        },
    },
    Repair {
        name: "definite_assignment_in_object",
        doc: "`{ a!: 1 }`: a definite assignment assertion where it is not permitted.",
        applies: |t, s, e, m| m == "Expected `,` or `}` but found `!`" && t.get(s..e) == Some("!"),
        run: |_, s, e| {
            member_marker(
                s,
                e,
                Code::DefiniteAssignmentNotPermitted,
                "A definite assignment assertion '!' is not permitted in this context.",
            )
        },
    },
    Repair {
        name: "enum_member_separator",
        doc: "`:` or `;` between enum members.",
        applies: |t, s, e, m| {
            matches!(
                m,
                "Expected `,` or `}` but found `:`" | "Expected `,` or `}` but found `;`"
            ) && matches!(t.get(s..e), Some(":" | ";"))
                && super::syntax_errors::inside_enum(t, s)
        },
        run: |_, s, e| enum_member_separator(s, e),
    },
    Repair {
        name: "return_outside_function",
        doc: "`return` at the top level.",
        applies: |_, _, _, m| m == "A 'return' statement can only be used within a function body.",
        run: |t, s, _| return_outside_function(t, s),
    },
    Repair {
        name: "catch_initializer",
        doc: "`catch (e = 1)`.",
        applies: |_, _, _, m| m == "Expected `)` but found `=`",
        run: |t, s, _| catch_initializer(t, s),
    },
    Repair {
        name: "rest_parameter_initializer",
        doc: "`(...a = [])`.",
        applies: |_, _, _, m| m == "Expected `,` or `)` but found `=`",
        run: |t, s, _| rest_parameter_initializer(t, s),
    },
    Repair {
        name: "index_signature_initializer",
        doc: "`[key: string] = 1`.",
        applies: |_, _, _, m| m == "Expected `,` or `]` but found `=`",
        run: |t, s, _| index_signature_initializer(t, s),
    },
    // A missing operand comes before a stray brace: `export = }` is TS1109 for the `}`, not a
    // brace left over after a statement.
    Repair {
        name: "unexpected_token",
        doc: "A missing operand, then a stray closing brace.",
        applies: |_, _, _, m| m == "Unexpected token",
        run: |t, s, e| {
            empty_declaration_list(t, s)
                .or_else(|| expression_expected(t, s))
                .or_else(|| stray_closing_brace(t, s, e))
        },
    },
    Repair {
        name: "rest_element_not_last",
        doc: "A rest element before other elements of a destructuring pattern.",
        applies: |_, _, _, m| m == "A rest element must be last in a destructuring pattern",
        run: |t, s, _| rest_element_not_last(t, s),
    },
    Repair {
        name: "rest_parameter_not_last",
        doc: "A rest parameter before other parameters.",
        applies: |_, _, _, m| m == "A rest parameter must be last in a parameter list",
        run: |t, s, _| rest_parameter_not_last(t, s),
    },
    Repair {
        name: "assignment_target",
        doc: "An assignment to something that cannot be assigned to.",
        applies: |_, _, _, m| m == "Cannot assign to this expression",
        run: assignment_target,
    },
    Repair {
        name: "enum_numeric_name",
        doc: "An enum member named by a number.",
        applies: |_, _, _, m| m == "An enum member cannot have a numeric name.",
        run: enum_numeric_name,
    },
    Repair {
        name: "enum_computed_name",
        doc: "An enum member named by a computed expression.",
        applies: |_, _, _, m| m == "Computed property names are not allowed in enums.",
        run: enum_computed_name,
    },
];

fn rule_for(text: &str, start: usize, end: usize, message: &str) -> Option<Rule> {
    let repair = REPAIRS
        .iter()
        .find(|repair| (repair.applies)(text, start, end, message))?;
    (repair.run)(text, start, end)
}

#[cfg(test)]
mod repair_table_tests {
    use super::*;

    #[test]
    fn repairs_have_unique_names_and_docs() {
        let mut seen = std::collections::HashSet::new();
        for repair in REPAIRS {
            assert!(seen.insert(repair.name), "duplicate repair {}", repair.name);
            assert!(!repair.doc.is_empty());
        }
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
    // `await using [a] = x` and `using [a] = x` are declarations that oxc read as an index.
    if left.starts_with("using") || left.starts_with("await using") {
        return None;
    }
    let before = text.get(..start)?.trim_end();
    let after = text.get(end..)?.trim_start();
    if before.ends_with("++")
        || before.ends_with("--")
        || after.starts_with("++")
        || after.starts_with("--")
    {
        return update_operand(left, start, end);
    }
    if let Some(rule) = for_head(text, left, start, end) {
        return Some(rule);
    }
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

// `function f() { static x = 1; }`, `{ private y = 1; }`: a class member modifier where a statement
// belongs. tsc reports TS1128 on the modifier and reads what follows as a statement, so the
// modifier goes.
fn member_modifier_statement(text: &str, start: usize) -> Option<Rule> {
    let before = text.get(..start)?.trim_end();
    const MODIFIERS: [&str; 5] = ["static", "public", "private", "protected", "readonly"];
    let modifier = MODIFIERS.into_iter().find(|m| before.ends_with(m))?;
    let head = &before[..before.len() - modifier.len()];
    if head.bytes().next_back().is_some_and(is_ident_byte) {
        return None;
    }
    let statement_start = head.trim_end().bytes().next_back();
    if !matches!(statement_start, None | Some(b'{' | b';' | b'}')) {
        return None;
    }
    let from = head.len();
    let to = before.len();
    let note = |start, end| Note {
        message: messages::syntax(
            Code::DeclarationOrStatementExpected,
            "Declaration or statement expected.".to_string(),
        ),
        start,
        end,
    };
    // Inside a method or constructor tsc ends the body at the modifier and the modifier starts a
    // class member, so the body's own closing brace is left over (see `stray_closing_brace`). Any
    // other block inside a class reads the modifier as a statement, which is not repaired here.
    let headers = open_block_headers(head);
    if headers.iter().any(|header| has_word(header, "class")) {
        let member_body = headers.len() >= 2
            && has_word(&headers[headers.len() - 2], "class")
            && !has_word(&headers[headers.len() - 1], "class");
        if !member_body {
            return None;
        }
        return Some(Rule {
            start: from,
            end: from,
            with: "} ".to_string(),
            copy: None,
            note: note(from, to),
        });
    }
    Some(Rule {
        start: from,
        end: to,
        with: String::new(),
        copy: None,
        note: note(from, to),
    })
}

// The header (the text since the previous `{`, `}` or `;`) of every block still open at the end of
// `before`, outermost first. Strings and comments are not skipped, which is enough for the
// statement-level shapes this is asked about.
fn open_block_headers(before: &str) -> Vec<&str> {
    let mut open = Vec::new();
    let mut header_start = 0;
    for (index, byte) in before.bytes().enumerate() {
        match byte {
            b'{' => {
                open.push(&before[header_start..index]);
                header_start = index + 1;
            }
            b'}' => {
                open.pop();
                header_start = index + 1;
            }
            b';' => header_start = index + 1,
            _ => {}
        }
    }
    open
}

fn has_word(text: &str, word: &str) -> bool {
    text.match_indices(word).any(|(at, _)| {
        let before = text[..at].bytes().next_back();
        let after = text.as_bytes().get(at + word.len()).copied();
        !before.is_some_and(is_ident_byte) && !after.is_some_and(is_ident_byte)
    })
}

// A `}` that closes nothing: tsc reports TS1128 on it and carries on without it.
fn stray_closing_brace(text: &str, start: usize, end: usize) -> Option<Rule> {
    if text.get(start..end) != Some("}") || !open_block_headers(text.get(..start)?).is_empty() {
        return None;
    }
    // A `}` straight after an operand supplied for a missing one (`export = }`) is the same
    // error tsc reported as TS1109; it does not report the brace again.
    if text.get(..start)?.trim_end().ends_with("(0 as any)") {
        return None;
    }
    Some(Rule {
        start,
        end,
        with: String::new(),
        copy: None,
        note: Note {
            message: messages::syntax(
                Code::DeclarationOrStatementExpected,
                "Declaration or statement expected.".to_string(),
            ),
            start,
            end,
        },
    })
}

// `(...arg: number[] = []) => 1`: tsc reports TS1048 on the rest parameter's name and drops the
// initializer.
fn rest_parameter_initializer(text: &str, start: usize) -> Option<Rule> {
    if text.get(start..start + 1)? != "=" {
        return None;
    }
    let before = text.get(..start)?;
    let mut depth = 0usize;
    let mut param_start = None;
    for (index, byte) in before.bytes().enumerate().rev() {
        match byte {
            b')' | b']' | b'}' | b'>' => depth += 1,
            b'(' | b'[' | b'{' | b'<' if depth > 0 => depth -= 1,
            b'(' | b',' if depth == 0 => {
                param_start = Some(index + 1);
                break;
            }
            _ => {}
        }
    }
    let param = &before[param_start?..];
    let dots = param.trim_start();
    let rest = dots.strip_prefix("...")?;
    let name = rest.trim_start();
    let name_len = name.bytes().take_while(|b| is_ident_byte(*b)).count();
    if name_len == 0 {
        return None;
    }
    let name_start = before.len() - name.len();
    let mut depth = 0usize;
    let mut init_end = None;
    for (offset, byte) in text.bytes().enumerate().skip(start + 1) {
        match byte {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b',' if depth == 0 => {
                init_end = Some(offset);
                break;
            }
            b')' | b']' | b'}' => depth = depth.checked_sub(1)?,
            _ => {}
        }
    }
    let init_end = init_end?;
    if text[start + 1..init_end].trim().is_empty() {
        return None;
    }
    Some(Rule {
        start: before.trim_end().len(),
        end: init_end,
        with: String::new(),
        copy: None,
        note: Note {
            message: messages::syntax(
                Code::RestParameterInitializer,
                "A rest parameter cannot have an initializer.".to_string(),
            ),
            start: name_start,
            end: name_start + name_len,
        },
    })
}

// `return foo;` outside any function: tsc reports TS1108 on `return` and returns from checking the
// statement, so `foo` is never looked at. The expression goes, so it is not checked here either.
fn return_outside_function(text: &str, start: usize) -> Option<Rule> {
    let rest = text.get(start..)?.strip_prefix("return")?;
    if rest.starts_with(is_ident_byte_char) {
        return None;
    }
    let mut depth = 0usize;
    let mut expr_end = rest.len();
    for (offset, byte) in rest.bytes().enumerate() {
        match byte {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' if depth == 0 => {
                expr_end = offset;
                break;
            }
            b')' | b']' | b'}' => depth -= 1,
            b';' | b'\n' if depth == 0 => {
                expr_end = offset;
                break;
            }
            _ => {}
        }
    }
    if rest[..expr_end].trim().is_empty() {
        return None;
    }
    let from = start + "return".len();
    Some(Rule {
        start: from,
        end: from + expr_end,
        with: String::new(),
        copy: None,
        note: Note {
            message: messages::syntax(
                Code::ReturnOutsideFunction,
                "A 'return' statement can only be used within a function body.".to_string(),
            ),
            start,
            end: from,
        },
    })
}

fn is_ident_byte_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '$'
}

fn as_any_target(left: &str) -> String {
    format!("({left} as any).__tsox_repair")
}

// `1 ++`, `--1`, `++(2)`: tsc reports TS2357 for a number, which is the right type for the
// operator, and TS2356 (never TS2357) for a boolean or string, whose type is wrong. Any other
// operand's type is not known from the text, so it is left alone.
fn update_operand(left: &str, start: usize, end: usize) -> Option<Rule> {
    let operand = left.trim_matches(|c| c == '(' || c == ')' || c == ' ');
    let quoted = operand.len() >= 2
        && matches!(operand.as_bytes()[0], b'"' | b'\'')
        && operand.ends_with(operand.chars().next()?);
    let (code, text) = if !operand.is_empty()
        && operand.bytes().all(|b| b.is_ascii_digit() || b == b'.')
    {
        (
            Code::IncrementOperandNotReference,
            "The operand of an increment or decrement operator must be a variable or a property access.",
        )
    } else if quoted || operand == "true" || operand == "false" {
        (
            Code::IncrementOperandNotNumeric,
            "An arithmetic operand must be of type 'any', 'number', 'bigint' or an enum type.",
        )
    } else {
        return None;
    };
    Some(Rule {
        start,
        end,
        with: as_any_target(left),
        copy: Some((1, 0, left.len())),
        note: Note {
            message: messages::syntax(code, text.to_string()),
            start,
            end,
        },
    })
}

// `for (f() in x)`, `for (new C() of x)`: tsc reports TS2406 or TS2487 on the head, which here is a
// call or `new` expression. Other heads (`this`, `a = 1`) get codes that depend on types.
fn for_head(text: &str, left: &str, start: usize, end: usize) -> Option<Rule> {
    let before = text.get(..start)?.trim_end().strip_suffix('(')?;
    let before = before.trim_end();
    let before = before.strip_suffix("await").map_or(before, str::trim_end);
    before.strip_suffix("for")?;
    let after = text.get(end..)?.trim_start();
    let (code, text) = if after.starts_with("in ") || after.starts_with("in(") {
        (
            Code::ForInTargetInvalid,
            "The left-hand side of a 'for...in' statement must be a variable or a property access.",
        )
    } else if after.starts_with("of ") || after.starts_with("of(") {
        (
            Code::ForOfTargetInvalid,
            "The left-hand side of a 'for...of' statement must be a variable or a property access.",
        )
    } else {
        return None;
    };
    if !(left.ends_with(')') || left.starts_with("new ")) {
        return None;
    }
    Some(Rule {
        start,
        end,
        with: as_any_target(left),
        copy: Some((1, 0, left.len())),
        note: Note {
            message: messages::syntax(code, text.to_string()),
            start,
            end,
        },
    })
}

// `let [...a, b] = x`, `let {...a, b} = x`: tsc reports TS2462 on the rest name and binds the rest
// to what is left over after the other elements, so the rest element moves to the end.
fn rest_element_not_last(text: &str, start: usize) -> Option<Rule> {
    let rest = text.get(start..)?.strip_prefix("...")?;
    let name_len = rest.bytes().take_while(|b| is_ident_byte(*b)).count();
    if name_len == 0 || rest.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let name = &rest[..name_len];
    let after_name = start + 3 + name_len;
    let tail = text.get(after_name..)?;
    let comma = tail.find(',')?;
    if !tail[..comma].trim().is_empty() {
        return None;
    }
    let mut depth = 0usize;
    let mut close = None;
    for (offset, byte) in text.bytes().enumerate().skip(after_name + comma + 1) {
        match byte {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' if depth == 0 => {
                close = Some(offset);
                break;
            }
            b')' | b']' | b'}' => depth -= 1,
            _ => {}
        }
    }
    let close = close?;
    let others = text[after_name + comma + 1..close]
        .trim()
        .trim_end_matches(',')
        .trim();
    if others.is_empty() || others.contains("...") {
        return None;
    }
    Some(Rule {
        start,
        end: close,
        with: format!("{others}, ...{name}"),
        copy: None,
        note: Note {
            message: messages::syntax(
                Code::RestElementNotLast,
                "A rest element must be last in a destructuring pattern.".to_string(),
            ),
            start: start + 3,
            end: after_name,
        },
    })
}

// `var f = () => ;`, `var a = 1 + ;`, `(2 * )`: an operator or arrow with no operand before a token that
// ends the expression. tsc reports TS1109 on that token and goes on with a missing operand, so
// one is supplied here: `(0 as any)` has type any, which checks without any error of its own.
fn expression_expected(text: &str, start: usize) -> Option<Rule> {
    let next = text.get(start..)?.chars().next()?;
    if !matches!(next, ';' | ')' | ']' | '}') {
        return None;
    }
    let before = text.get(..start)?.trim_end();
    if before.ends_with("++") || before.ends_with("--") || before.ends_with("*/") {
        return None;
    }
    const OPERAND_EXPECTED: [&str; 12] =
        ["=>", "=", "+", "-", "*", "/", "%", "&", "|", "^", "!", "~"];
    if !OPERAND_EXPECTED.iter().any(|op| before.ends_with(op)) {
        return None;
    }
    // A `*` opening a member (`{ * }`, `static *`) is a generator with no name, which tsc reports
    // as TS1003, not a multiplication missing its right side.
    if let Some(rest) = before.strip_suffix('*') {
        let rest = rest.trim_end();
        if rest.is_empty()
            || rest.ends_with(['{', '}', ';', ','])
            || rest.ends_with("static")
            || rest.ends_with("async")
        {
            return None;
        }
    }
    Some(Rule {
        copy: None,
        start,
        end: start,
        with: "(0 as any)".to_string(),
        note: Note {
            message: messages::syntax(Code::ExpressionExpected, "Expression expected.".to_string()),
            start,
            end: start + next.len_utf8(),
        },
    })
}

// `var ;`, `const` at the end of a file: tsc reports TS1123 where the first declarator should be and
// carries on, so one is supplied: a uniquely named `any` constant that checks without errors.
fn empty_declaration_list(text: &str, start: usize) -> Option<Rule> {
    let next = text.get(start..)?.chars().next();
    if !matches!(next, None | Some(';' | '}')) {
        return None;
    }
    let before = text.get(..start)?.trim_end();
    let keyword = ["var", "const"].into_iter().find(|k| before.ends_with(k))?;
    let head = &before[..before.len() - keyword.len()];
    if head.bytes().next_back().is_some_and(is_ident_byte) {
        return None;
    }
    Some(Rule {
        copy: None,
        start,
        end: start,
        with: format!(" _tsox_missing_{start} = 0 as any"),
        note: Note {
            message: messages::syntax(
                Code::VariableDeclarationListEmpty,
                "Variable declaration list cannot be empty.".to_string(),
            ),
            // tsc reports it with no width, straight after the keyword.
            start: before.len(),
            end: before.len(),
        },
    })
}
