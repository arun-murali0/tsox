use oxc_span::Span;

use crate::arena::{TypeArena, TypeId};
use crate::diagnostic_messages::DiagnosticMessage;
use crate::diagnostics::{Diagnostic, Severity};
use crate::fxhash::FxHashMap;
use crate::keys::DeclKey;
use crate::namespace::TypeNamespace;
use crate::portable::{ImportSource, Importer};
use crate::semantic::SemanticQueries;
use crate::semantic::queries::RelationCache;
use crate::symbol_map::SymbolTypeMap;
use crate::types::FileId;
use crate::unit::CancelToken;
use oxc_semantic::SymbolId;

use super::narrow::{JumpFrame, NarrowState, WriteMap};

// The shared mutable state for checking one program. Every feature module
// (expressions, statements, narrowing) takes &mut CheckContext rather than owning
// its own state, by design: this keeps a single-threaded check simple today while
// making the eventual unit of parallelism explicit later, one independent
// CheckContext per file, rather than fine-grained locking inside one file's check.
// See docs/architecture.md for the fuller rationale.
pub struct CheckContext<'ast, 'src> {
    pub arena: TypeArena,
    pub namespace: TypeNamespace<'ast>,
    pub symbols: SymbolTypeMap,
    pub diagnostics: Vec<Diagnostic>,
    pub file_name: &'src str,

    pub narrow: NarrowState,

    // Memoizes is_subtype, is_assignable and is_disjoint answers by relation and the
    // exact TypeId pair asked about, for the lifetime of this one file's check. The
    // pair is ordered, never symmetrized for subtyping: subtyping is not symmetric
    // (see subtyping.rs), so (a, b) and (b, a) are independent entries.
    //
    // The cache is tied to the arena's generation (see RelationCache). Resolving a
    // declaration's Ref with TypeArena::resolve_ref changes what an existing id means,
    // and the next query drops every answer from before it, so nothing stale can be
    // read back.
    //
    // This only catches repeats of the *same* TypeId pair. alloc() reuses one TypeId
    // for identical anonymous composites (so the same-shaped object literal at two
    // call sites shares an entry here), but a named type, a union or a
    // declaration's Ref has its own id and its own entries even when it
    // matches another by shape. What it does catch: the same subterm pair reached from
    // different branches of one recursive object or union comparison, and the
    // (candidate, existing) pairs generic inference compares, so a call like
    // `allSame(1, 2, ..., 8)` computes each pair once.
    //
    // type_annotation::check_type_argument_constraint does not use it, because that
    // function receives only the namespace and the arena, not this context. The
    // generation check would make a cached answer safe there too; it stays uncached
    // because threading the cache through a resolution-time call has not been needed.
    pub relation_cache: RelationCache,

    pub current_return_type: Option<TypeId>,
    // True while the body of an `async` function is walked. What its `return` statements
    // give is checked against the awaited type, since the annotation says Promise<T> but
    // the body returns a T.
    pub current_return_async: bool,
    // Problem: a function with no return annotation has no declared type to check its
    // `return` statements against, and none to give its callers.
    // Picked: while such a function's body is walked, every returned type lands here,
    // and enter_return_scope / leave_return_scope keep it per function, so a nested
    // function, arrow or method never adds its returns to the enclosing one.
    // Cost: a caller checked before the body is walked sees the placeholder `any`.
    pub inferred_returns: Option<Vec<TypeId>>,

    // Problem: a function with no return annotation takes its return type from its body,
    // and a call to another such function inside a `return` needs that function's type
    // too. Two functions that each need the other's have no answer, and the placeholder
    // `any` they get was passed off as one without a word.
    // Picked: while an unannotated function's `return` expressions are checked, each call
    // to a function declaration is noted as an edge from that function to the callee. A
    // cycle among those edges is reported once for each function on it (TS7023).
    // Cost: only calls by name to a function declared in the same file are seen. A call
    // through an import, a method or a variable is not, so a cycle through one is not
    // reported and still reads as `any`.
    pub return_reads: ReturnReads,

    // Inferred signatures for anonymous default function declarations, keyed by their
    // source start offset. They have no SymbolId, but their bodies still need checking
    // before the portable export table is written.
    pub exported_function_types: FxHashMap<u32, TypeId>,

    pub current_class_instance: Option<TypeId>,

    // True while checking the body of a function whose `this` has no type at all:
    // a plain function expression with no `this` parameter and no contextual
    // type. A `this` used there is an implicit any, which tsc reports under
    // noImplicitThis. Left false everywhere else, including object literal
    // methods (whose `this` is the literal) and any function this checker cannot
    // yet prove has no contextual `this`, so those stay silent.
    pub implicit_this: bool,

    // A one-shot request from the caller of infer_function_expression_type: the
    // very next function expression checked is known to have no contextual
    // `this`, so its body should run with implicit_this set. Consumed on entry.
    pub next_function_has_no_this: bool,
    // The declared type the very next expression is being checked against, handed down by
    // a caller that has one (an annotated variable, a return type, a parameter). Only an
    // object or array literal reads it, to keep a literal property whose declared type is
    // a literal; infer_expression_type takes it on entry, so it never reaches a deeper
    // expression by accident.
    pub contextual: Option<TypeId>,

    // The type of each branch of every conditional expression checked so far, by the
    // expression's start offset. tsc reports a ternary that does not fit its target
    // once per branch that does not fit; the branches' types cannot be recovered from
    // the union the whole expression has, and inferring them again would report their
    // own errors twice, so they are kept when they are first inferred.
    pub conditional_arms: std::collections::HashMap<u32, (TypeId, TypeId)>,

    // The `break` and `continue` targets the walker is currently inside, innermost last.
    // A jump records the narrowing in force where it happens into its target's frame, so
    // the code after a loop (and the head of its next pass) can join those states in
    // instead of only seeing the state at the end of the body. See control_flow.rs.
    pub jump_frames: Vec<JumpFrame>,

    // The label of a `label: while (...)` waiting for its loop, so a `continue label` can
    // find the loop. Set by the labeled statement and consumed by the loop it labels.
    pub pending_loop_label: Option<String>,

    // Where each variable is written, collected once per file by check_top_level. Empty
    // until then, which makes every loop and closure behave as if nothing were assigned.
    pub writes: WriteMap,

    // Set when a unit is checked under a token. The flag is read once every
    // CANCEL_CHECK_INTERVAL statements rather than on each one, because statements are
    // the hottest thing the walker visits and an atomic load per statement would show.
    // Once it has seen the token cancelled, `cancelled` stays true and every later
    // statement returns at once, which unwinds the walk without a panic or an error type
    // threaded through every checker function.
    pub cancel: Option<CancelToken>,
    pub cancelled: bool,
    // Set once the stack guard has stopped a descent, so one file gives one diagnostic
    // for it however many branches of the nesting hit the limit.
    nesting_reported: bool,
    statements_seen: u32,

    // Where this file's imports come from, when it is checked as part of a project. None
    // for a single-file check, which then leaves its import declarations alone.
    pub imports: Option<&'src dyn ImportSource>,
    // Rebuilds the imported declarations in this check's arena. One per check: it
    // remembers which slot stands for which key, so two uses of an imported declaration
    // share one id, and it is the key resolver for anything imported that the file
    // exports again.
    pub importer: Importer,
    // Each imported local name and the declarations it was bound to, so `export { X }`
    // of an import can be answered with the key the declaration already has. Only ever
    // looked up by name, never walked.
    pub imported_names: FxHashMap<String, Vec<DeclKey>>,

    // How many ambient blocks (`declare namespace`, `declare module "x" { }`) the walker
    // is inside. Zero outside any. tsc treats every declaration in such a block as ambient
    // without its own `declare`, so a class there has no property initializers to demand
    // (TS2564) and a private member of it is not an implicit-any report (TS7006).
    pub ambient_depth: u32,

    /// Global value bindings exposed by `export as namespace` in a project dependency.
    /// These are consulted only when Oxc has no lexical symbol for the identifier, so a
    /// local declaration still shadows a UMD global normally.
    pub global_values: FxHashMap<String, TypeId>,
}

const CANCEL_CHECK_INTERVAL: u32 = 32;

// What a function body replaces on entry and gives back on exit, so the two pieces
// of return bookkeeping can never be restored out of step with each other.
pub struct ReturnScope {
    outer_type: Option<TypeId>,
    outer_inferred: Option<Vec<TypeId>>,
    outer_async: bool,
    outer_owner: Option<SymbolId>,
}

// The record of which function declarations an unannotated function's return expressions
// call (see `CheckContext::return_reads`).
#[derive(Default)]
pub struct ReturnReads {
    // The function whose return type is being inferred from the body being walked. None in
    // any nested function, arrow or method, which has its own scope and its own answer.
    pub owner: Option<SymbolId>,
    // True while the expression of a `return` statement is being checked.
    pub in_return_expression: bool,
    edges: Vec<(SymbolId, SymbolId)>,
    names: FxHashMap<SymbolId, (String, Span)>,
    reported: Vec<SymbolId>,
}

impl ReturnReads {
    // A call by name to the function declaration `callee`, seen where it can affect the
    // return type of the function being inferred.
    pub fn note_call(&mut self, callee: SymbolId) {
        if !self.in_return_expression {
            return;
        }
        if let Some(owner) = self.owner
            && !self.edges.contains(&(owner, callee))
        {
            self.edges.push((owner, callee));
        }
    }

    pub fn note_owner(&mut self, symbol: SymbolId, name: &str, span: Span) {
        self.owner = Some(symbol);
        self.names
            .entry(symbol)
            .or_insert_with(|| (name.to_string(), span));
    }

    // The functions on a cycle through `symbol` that have not been reported yet, with the
    // name and place to report each at. Empty when `symbol` is on no cycle.
    pub fn take_cycle(&mut self, symbol: SymbolId) -> Vec<(String, Span)> {
        let reach = |from: SymbolId, forward: bool| -> Vec<SymbolId> {
            let mut seen: Vec<SymbolId> = Vec::new();
            let mut pending = vec![from];
            while let Some(node) = pending.pop() {
                for (a, b) in &self.edges {
                    let (here, next) = if forward { (*a, *b) } else { (*b, *a) };
                    if here == node && !seen.contains(&next) {
                        seen.push(next);
                        pending.push(next);
                    }
                }
            }
            seen
        };
        let ahead = reach(symbol, true);
        let behind = reach(symbol, false);
        let mut found = Vec::new();
        for member in ahead.into_iter().filter(|node| behind.contains(node)) {
            if self.reported.contains(&member) {
                continue;
            }
            self.reported.push(member);
            if let Some((name, span)) = self.names.get(&member) {
                found.push((name.clone(), *span));
            }
        }
        found.sort_by_key(|(_, span)| span.start);
        found
    }
}

impl<'ast, 'src> CheckContext<'ast, 'src> {
    // `infer` is true only for a function declaration with no return annotation, the one
    // place that collects what its body returns. Every other function leaves the
    // collection off, which also hides the enclosing function's list from its body.
    pub fn enter_return_scope(&mut self, return_type: Option<TypeId>, infer: bool) -> ReturnScope {
        ReturnScope {
            outer_type: std::mem::replace(&mut self.current_return_type, return_type),
            outer_inferred: std::mem::replace(&mut self.inferred_returns, infer.then(Vec::new)),
            outer_async: std::mem::replace(&mut self.current_return_async, false),
            outer_owner: self.return_reads.owner.take(),
        }
    }

    // enter_return_scope for a function that may be `async`. The annotation of an async
    // function is Promise<T>, but its body returns a T, so the body is checked against T.
    pub fn enter_function_return_scope(
        &mut self,
        declared: Option<TypeId>,
        infer: bool,
        is_async: bool,
    ) -> ReturnScope {
        let expected = declared.map(|ty| if is_async { self.promised_type(ty) } else { ty });
        let scope = self.enter_return_scope(expected, infer);
        self.current_return_async = is_async;
        scope
    }

    // `Promise<T>` as T; any other type is left as it is.
    pub fn promised_type(&self, declared: TypeId) -> TypeId {
        match self.arena.app_parts(declared) {
            Some(("Promise", [inner])) => *inner,
            _ => declared,
        }
    }

    // What `await` gives for a type: every Promise layer comes off.
    pub fn awaited_type(&self, ty: TypeId) -> TypeId {
        let mut ty = ty;
        while let Some(("Promise", [inner])) = self.arena.app_parts(ty) {
            ty = *inner;
        }
        ty
    }

    // Restores the enclosing function's state and hands back the types this function
    // returned (empty unless it was entered with `infer`).
    pub fn leave_return_scope(&mut self, scope: ReturnScope) -> Vec<TypeId> {
        self.current_return_type = scope.outer_type;
        self.current_return_async = scope.outer_async;
        self.return_reads.owner = scope.outer_owner;
        std::mem::replace(&mut self.inferred_returns, scope.outer_inferred).unwrap_or_default()
    }

    // Takes an arena the caller already owns, so a session can hand the same
    // allocation to each check in turn. The arena must already be cleared: this does
    // not reset it, because the caller is the one who knows whether it is reusing one.
    pub fn with_arena_and_file_id(file_name: &'src str, file_id: FileId, arena: TypeArena) -> Self {
        Self {
            arena,
            namespace: TypeNamespace::with_file_id(file_id),
            symbols: SymbolTypeMap::new(),
            diagnostics: Vec::new(),
            file_name,
            narrow: NarrowState::new(),
            relation_cache: RelationCache::default(),
            current_return_type: None,
            current_return_async: false,
            inferred_returns: None,
            return_reads: ReturnReads::default(),
            exported_function_types: FxHashMap::default(),
            current_class_instance: None,
            implicit_this: false,
            next_function_has_no_this: false,
            contextual: None,
            conditional_arms: std::collections::HashMap::new(),
            jump_frames: Vec::new(),
            pending_loop_label: None,
            writes: WriteMap::default(),
            cancel: None,
            cancelled: false,
            nesting_reported: false,
            statements_seen: 0,
            imports: None,
            importer: Importer::new(),
            imported_names: FxHashMap::default(),
            ambient_depth: 0,
            global_values: FxHashMap::default(),
        }
    }

    // True once the unit has been cancelled; the statement walker asks before each
    // statement and stops when it is.
    pub fn should_stop(&mut self) -> bool {
        if self.cancelled {
            return true;
        }
        self.statements_seen = self.statements_seen.wrapping_add(1);
        if self.statements_seen.is_multiple_of(CANCEL_CHECK_INTERVAL)
            && self.cancel.as_ref().is_some_and(CancelToken::is_cancelled)
        {
            self.cancelled = true;
        }
        self.cancelled
    }

    // Problem: a type declared in the namespace being checked was written `Test.ILineTokens`
    // in a message, where tsc writes `ILineTokens`: inside a namespace its members are
    // reachable by their short name, and tsc prints the shortest name that resolves there.
    // Picked: drop every enclosing namespace prefix from the finished text, a prefix only
    // when it starts a name (not when it ends another identifier or path).
    // Cost: a member of an enclosing namespace that a local name hides would also lose its
    // prefix, which tsc keeps.
    fn message_text(&self, text: String) -> String {
        let mut text = text;
        let mut prefixes: Vec<&String> = self.namespace.scope_prefixes().iter().collect();
        prefixes.sort_by_key(|prefix| std::cmp::Reverse(prefix.len()));
        for prefix in prefixes {
            let needle = format!("{prefix}.");
            let mut out = String::with_capacity(text.len());
            let mut rest = text.as_str();
            while let Some(at) = rest.find(&needle) {
                let before = rest[..at].chars().next_back();
                let starts_a_name = before
                    .is_none_or(|c| !(c.is_alphanumeric() || c == '_' || c == '$' || c == '.'));
                out.push_str(&rest[..at]);
                if !starts_a_name {
                    out.push_str(&needle);
                }
                rest = &rest[at + needle.len()..];
            }
            out.push_str(rest);
            text = out;
        }
        text
    }

    // Called where the stack guard says a recursive walk must stop: reports once per file
    // and hands back the error type, so the caller carries on with an unknown type. It is a
    // warning, like the other "not checked" notices, because the code is fine and has no tsc
    // counterpart: an error would fail the run and show up as an extra error in a compare.
    pub fn nesting_too_deep(&mut self, span: Span) -> TypeId {
        if !self.nesting_reported {
            self.nesting_reported = true;
            self.warning(
                crate::diagnostic_messages::messages::nesting_too_deep(),
                span,
            );
        }
        self.arena.error()
    }

    pub fn error(&mut self, message: DiagnosticMessage, span: Span) {
        let text = self.message_text(message.text);
        self.diagnostics.push(Diagnostic {
            severity: Severity::Error,
            code: message.code,
            message: text,
            file_name: self.file_name.to_string(),
            start: span.start,
            end: span.end,
        });
    }

    // The seam between AST-facing bridge code and the semantic core. Bridge code
    // calls ctx.semantic().is_assignable(...) instead of reaching into subtyping
    // directly, so TypeScript-specific assignability rules that are not plain
    // structural subtyping, such as excess property checks on object literals or
    // const assertions, have one place to live later without changing call sites.
    //
    // Takes &mut self (not &self) because SemanticQueries carries a mutable
    // handle to relation_cache alongside the arena. Field-level destructuring
    // here borrows the two fields disjointly, but that disjointness is only
    // visible inside this function body; past this call boundary the returned
    // SemanticQueries opaquely holds part of `self`, so callers cannot access
    // ctx.arena or ctx.relation_cache directly while a SemanticQueries from this
    // call is still alive. In practice this means any TypeId needed as an
    // argument (e.g. ctx.arena.number()) must be read into a local *before*
    // calling ctx.semantic(), not inline as part of the same expression.
    pub fn semantic(&mut self) -> SemanticQueries<'_> {
        SemanticQueries::new(&self.arena, &mut self.relation_cache)
    }

    // Problem: `<T>(x: T) => T` passed where `(x: number) => number` is expected was
    // rejected, because the comparison sees an unbound T against number. tsc first
    // instantiates the generic source against the target's parameter types.
    // Picked: infer the source's own type parameters from the target's parameter types,
    // fill any left over from their default, constraint or unknown, and compare the
    // instantiated signature. Only the check sees the instantiated type; messages keep
    // the declared one.
    // Cost: only a generic function at the top of the comparison is instantiated, not one
    // nested inside an object property or a union member, and a generic target is
    // compared as it is.
    pub fn is_assignable_instantiating(&mut self, source: TypeId, target: TypeId) -> bool {
        let source = self.instantiate_generic_source(source, target);
        self.semantic().is_assignable(source, target)
    }

    fn instantiate_generic_source(&mut self, source: TypeId, target: TypeId) -> TypeId {
        use crate::types::{FunctionType, Param, Type};
        let (Type::Function(from), Type::Function(to)) =
            (self.arena.get(source), self.arena.get(target))
        else {
            return source;
        };
        if from.type_params.is_empty() || !to.type_params.is_empty() {
            return source;
        }
        let (from, to) = (from.clone(), to.clone());
        let own: Vec<_> = from.type_params.iter().map(|p| p.id).collect();
        let mut bindings = Vec::new();
        for position in 0..from.params.len().max(to.params.len()) {
            if let (Some(wanted), Some(given)) = (
                crate::subtyping::param_type_at(&self.arena, &from.params, position),
                crate::subtyping::param_type_at(&self.arena, &to.params, position),
            ) {
                crate::semantic::GenericSolver::infer_type_param_bindings(
                    &mut self.arena,
                    wanted,
                    given,
                    &mut bindings,
                    &[],
                    &mut self.relation_cache,
                );
            }
        }
        bindings.retain(|(id, _)| own.contains(id));
        // An inferred type that breaks its parameter's `extends` bound falls back to the
        // bound, which is what makes `<T extends string>(x: T)` fail against `(x: number)`.
        for parameter in &from.type_params {
            let Some(constraint) = parameter.constraint else {
                continue;
            };
            let Some(slot) = bindings.iter().position(|(id, _)| *id == parameter.id) else {
                continue;
            };
            let inferred = bindings[slot].1;
            if !self.semantic().is_assignable(inferred, constraint) {
                bindings[slot].1 = constraint;
            }
        }
        // Inference from an `any` parameter yields any, so a parameter nothing else informed
        // takes it instead of unknown.
        let any = self.arena.any();
        let target_has_any = to.params.iter().any(|p| p.type_id == any);
        for parameter in &from.type_params {
            if !bindings.iter().any(|(id, _)| *id == parameter.id) {
                let fill = if target_has_any {
                    any
                } else {
                    parameter
                        .default
                        .or(parameter.constraint)
                        .unwrap_or_else(|| self.arena.unknown())
                };
                bindings.push((parameter.id, fill));
            }
        }
        let substitute = |arena: &mut TypeArena, id: TypeId| {
            crate::semantic::GenericSolver::substitute_bound_type_params(arena, id, &bindings)
        };
        let params: Vec<Param> = from
            .params
            .iter()
            .map(|p| Param {
                type_id: substitute(&mut self.arena, p.type_id),
                ..p.clone()
            })
            .collect();
        let return_type = substitute(&mut self.arena, from.return_type);
        self.arena.alloc(Type::Function(FunctionType {
            params,
            return_type,
            type_params: Vec::new(),
            is_untyped: from.is_untyped,
        }))
    }

    pub fn warning(&mut self, message: DiagnosticMessage, span: Span) {
        let text = self.message_text(message.text);
        self.diagnostics.push(Diagnostic {
            severity: Severity::Warning,
            code: message.code,
            message: text,
            file_name: self.file_name.to_string(),
            start: span.start,
            end: span.end,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> CheckContext<'static, 'static> {
        CheckContext::with_arena_and_file_id("t.ts", FileId::ROOT, TypeArena::new())
    }

    #[test]
    fn a_raised_token_is_seen_within_one_interval_and_stays_seen() {
        let token = CancelToken::new();
        let mut ctx = context();
        ctx.cancel = Some(token.clone());

        for _ in 0..CANCEL_CHECK_INTERVAL - 1 {
            assert!(!ctx.should_stop());
        }
        token.cancel();
        assert!(ctx.should_stop());
        assert!(ctx.cancelled);
        assert!(ctx.should_stop());
    }

    #[test]
    fn a_context_with_no_token_never_stops() {
        let mut ctx = context();
        for _ in 0..CANCEL_CHECK_INTERVAL * 4 {
            assert!(!ctx.should_stop());
        }
    }
}
