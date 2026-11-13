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
                });
                self.text.push_str(&self.original[at..edit.start]);
            }
            self.segments.push(Segment {
                cur_start: self.text.len(),
                cur_end: self.text.len() + edit.with.len(),
                orig_start: edit.start,
                orig_end: edit.end,
                replaced: true,
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
    while at > 0 && bytes[at - 1].is_ascii_whitespace() {
        at -= 1;
    }
    let chain_end = at;
    while at > 0 && is_name_byte(bytes[at - 1]) {
        at -= 1;
    }
    let chain = text.get(at..chain_end)?;
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

// `[key: string];` with no type: tsc reports TS1021 on the signature and gives it type `any`.
fn index_signature_without_type(text: &str, start: usize, end: usize) -> Option<Rule> {
    let signature = text.get(start..end)?;
    if !(signature.starts_with('[') && signature.ends_with(']')) {
        return None;
    }
    // tsc reports a bad key (`[k?: string]`, `[k: any]`) as TS1019/TS1268, not TS1021.
    if !super::syntax_errors::plain_index_key(text, oxc_span::Span::new(start as u32, end as u32)) {
        return None;
    }
    Some(Rule {
        start: end,
        end,
        with: ": any".to_string(),
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
