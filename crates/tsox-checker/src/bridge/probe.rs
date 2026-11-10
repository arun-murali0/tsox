//! Asking one unit what type is at an offset.
//!
//! A unit's types live in its arena and are gone when it ends, so a type cannot be asked of
//! a finished check. A [`ProbeRequest`] rides along with a check instead: while the unit's
//! bodies are walked, each expression that holds the offset offers its type, and the
//! innermost one is kept, already written out as text. A name at its declaration (`x` in
//! `const x = 1`, a parameter, a function) is not an expression; its type is read from the
//! unit's table of symbol types when the walk is done.
//!
//! Nothing here changes what a check finds. A unit with no request does none of this.

use std::sync::Mutex;

use oxc_ast::AstKind;
use oxc_semantic::Semantic;
use oxc_span::Span;

use crate::arena::{TypeArena, TypeId};
use crate::symbol_map::SymbolTypeMap;
use crate::type_display::display_type;
use crate::types::Type;

/// The type as text, or no text for the checker's own "error" type. That one is what an
/// expression gets when the checker could not type it (it has reported or chosen not to
/// report why); it is not a type a person wrote or would want to read.
pub(crate) fn text_of(arena: &TypeArena, type_id: TypeId) -> String {
    match arena.get(type_id) {
        Type::Error => String::new(),
        _ => display_type(arena, type_id),
    }
}

/// The type of the innermost thing that holds an offset, and where that thing is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeAtOffset {
    pub start: u32,
    pub end: u32,
    pub text: String,
}

/// A question for one unit: what is the type at `offset`? The answer is left here when the
/// unit has been checked; `None` there means nothing typed holds the offset.
#[derive(Debug)]
pub struct ProbeRequest {
    offset: u32,
    answer: Mutex<Option<TypeAtOffset>>,
}

impl ProbeRequest {
    pub fn new(offset: u32) -> Self {
        Self {
            offset,
            answer: Mutex::new(None),
        }
    }

    pub fn offset(&self) -> u32 {
        self.offset
    }

    /// The answer, once the unit has been checked.
    pub fn take(&self) -> Option<TypeAtOffset> {
        self.answer.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    pub(crate) fn answer(&self, found: Option<TypeAtOffset>) {
        *self.answer.lock().unwrap_or_else(|e| e.into_inner()) = found;
    }
}

/// What a check keeps while a request is open.
pub struct ProbeState {
    offset: u32,
    best: Option<TypeAtOffset>,
}

impl ProbeState {
    pub(crate) fn new(offset: u32) -> Self {
        Self { offset, best: None }
    }

    /// Whether a thing with this span would replace what is kept: it holds the offset, and it
    /// is smaller than what is kept. The inner expressions are typed before the outer ones,
    /// so of two spans that are the same the first stays.
    pub(crate) fn wants(&self, span: Span) -> bool {
        span.start <= self.offset
            && self.offset <= span.end
            && self
                .best
                .as_ref()
                .is_none_or(|best| span.end - span.start < best.end - best.start)
    }

    pub(crate) fn offer(&mut self, span: Span, text: String) {
        if self.wants(span) {
            self.best = Some(TypeAtOffset {
                start: span.start,
                end: span.end,
                text,
            });
        }
    }

    /// What was kept. A thing the checker could not type is kept too, with no text, so that a
    /// larger expression around it does not stand in for it; it is not an answer.
    pub(crate) fn into_answer(self) -> Option<TypeAtOffset> {
        self.best.filter(|best| !best.text.is_empty())
    }
}

/// Offers the type of a thing at `span` to the open request, if there is one and the thing
/// holds its offset. The places that type a name or a member themselves, without going through
/// the expression typer (a callee), call this so the request still sees them.
pub(crate) fn offer(ctx: &mut super::context::CheckContext<'_, '_>, span: Span, type_id: TypeId) {
    if ctx.probe.as_ref().is_some_and(|probe| probe.wants(span)) {
        let text = text_of(&ctx.arena, type_id);
        if let Some(probe) = ctx.probe.as_mut() {
            probe.offer(span, text);
        }
    }
}

/// The name at its declaration that holds the offset, with the type the unit gave its symbol.
pub(crate) fn declared_name(
    semantic: &Semantic<'_>,
    symbols: &SymbolTypeMap,
    arena: &TypeArena,
    offset: u32,
) -> Option<(Span, String)> {
    let mut best: Option<(Span, String)> = None;
    for node in semantic.nodes().iter() {
        let AstKind::BindingIdentifier(binding) = node.kind() else {
            continue;
        };
        let span = binding.span;
        if span.start > offset || offset > span.end {
            continue;
        }
        if best
            .as_ref()
            .is_some_and(|(kept, _)| kept.end - kept.start <= span.end - span.start)
        {
            continue;
        }
        let Some(symbol) = binding.symbol_id.get() else {
            continue;
        };
        let Some(type_id) = symbols.get(symbol) else {
            continue;
        };
        best = Some((span, text_of(arena, type_id)));
    }
    best
}
