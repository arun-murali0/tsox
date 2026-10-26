use oxc_ast::ast::IdentifierReference;
use oxc_semantic::Scoping;
use oxc_span::GetSpan;

use crate::arena::TypeId;

use super::super::context::CheckContext;
use super::super::narrow::resolve_symbol_id;

// The narrow overlay is checked before the symbol's own declared type, since a
// variable narrowed in the current branch, for example by an earlier if
// (typeof x === "string"), should read as that narrowed type here, not the wider
// type it was originally declared with.
pub(crate) fn resolve_identifier_type(
    ident: &IdentifierReference,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> TypeId {
    let Some(symbol_id) = resolve_symbol_id(ident, scoping) else {
        // `undefined` is a real value in JS, not a keyword, but it is also not a
        // declared binding anywhere: oxc's scope analysis correctly reports it as
        // unresolved, the same as it would for any other undeclared global. A
        // local named `undefined` (`let undefined = 1;`) shadows this, since that
        // case resolves to a real symbol above and never reaches here.
        if ident.name == "undefined" {
            return ctx.arena.undefined();
        }
        if let Some(&global_type) = ctx.global_values.get(ident.name.as_str()) {
            return global_type;
        }
        // Problem: a name the language provides without any declaration in the file
        // (`Error`, `console`, `Math`) is unresolved to oxc's scope analysis, so a
        // body that throws `new Error(...)` or logs reported "Cannot find name".
        // That was hidden while try/throw bodies were skipped and showed as soon as
        // they were checked.
        // Picked: these ambient globals read as `any`. This checker has no lib
        // declarations yet, so any is the honest answer: nothing is checked about
        // them, and nothing false is reported.
        // Cost: `Math.foo()` and `new Error(1, 2, 3)` are not caught.
        if is_ambient_global(&ident.name) {
            return ctx.arena.any();
        }
        // A project global whose declaration could not be carried over: it exists, but its
        // type is not known.
        if ctx.namespace.is_unmodelled_global(&ident.name) {
            return ctx.arena.any();
        }
        // Problem: `Iterator` or a user's own `interface I {}` used as a value read as "Cannot
        // find name", where tsc says the name only refers to a type (TS2693).
        // Picked: a name this checker knows as a type, or one of the library types that have
        // no value, gets TS2693.
        let message = if ctx.namespace.contains(&ident.name) || is_library_type_only(&ident.name) {
            crate::diagnostic_messages::messages::type_used_as_value(&ident.name)
        } else {
            crate::diagnostic_messages::messages::unresolved_identifier(&ident.name)
        };
        ctx.error(message, ident.span());
        return ctx.arena.error();
    };

    if let Some(narrowed) = ctx.narrow.get(symbol_id) {
        return narrowed;
    }

    match ctx.symbols.get(symbol_id) {
        Some(type_id) => type_id,
        // oxc's own scope analysis resolved this identifier to a real binding, so
        // this only happens if declare_top_level left that binding's type
        // unregistered, an unresolvable annotation, for instance. Warned rather
        // than silently defaulted, since it points at a gap in this checker
        // rather than a mistake in the source being checked.
        None => {
            tracing::warn!(
                name = %ident.name,
                "resolved symbol has no registered type, falling back to the error sentinel"
            );
            ctx.arena.error()
        }
    }
}

// Values the runtime provides in every script. A name is only here if a program can use
// it with no import and no declaration; anything else is a real unresolved name.
// Library names that are only types: using one as a value is TS2693, not an unknown name.
fn is_library_type_only(name: &str) -> bool {
    matches!(
        name,
        "Iterator"
            | "Iterable"
            | "IterableIterator"
            | "AsyncIterator"
            | "AsyncIterable"
            | "ArrayLike"
            | "PromiseLike"
            | "Partial"
            | "Required"
            | "Readonly"
            | "Record"
            | "Pick"
            | "Omit"
            | "Exclude"
            | "Extract"
            | "NonNullable"
            | "ReturnType"
            | "Parameters"
            | "InstanceType"
    )
}

// Stopgap until lib declarations are loaded (E6 in BUILD.md): the names tsc's default lib
// provides as values (ECMAScript built-ins and the common DOM globals). Each reads as `any`.
fn is_ambient_global(name: &str) -> bool {
    matches!(
        name,
        "Error"
            | "TypeError"
            | "RangeError"
            | "SyntaxError"
            | "ReferenceError"
            | "EvalError"
            | "URIError"
            | "AggregateError"
            | "console"
            | "Math"
            | "JSON"
            | "Object"
            | "Array"
            | "Promise"
            | "Date"
            | "Map"
            | "Set"
            | "WeakMap"
            | "WeakSet"
            | "RegExp"
            | "Number"
            | "String"
            | "Boolean"
            | "Symbol"
            | "BigInt"
            | "Reflect"
            | "Proxy"
            | "globalThis"
            | "Intl"
            | "Atomics"
            | "WebAssembly"
            | "arguments"
            | "Int8Array"
            | "Uint8Array"
            | "Uint8ClampedArray"
            | "Int16Array"
            | "Uint16Array"
            | "Int32Array"
            | "Uint32Array"
            | "Float32Array"
            | "Float64Array"
            | "BigInt64Array"
            | "BigUint64Array"
            | "ArrayBuffer"
            | "SharedArrayBuffer"
            | "DataView"
            | "WeakRef"
            | "FinalizationRegistry"
            | "Function"
            | "encodeURIComponent"
            | "decodeURIComponent"
            | "encodeURI"
            | "decodeURI"
            | "escape"
            | "unescape"
            | "eval"
            | "queueMicrotask"
            | "structuredClone"
            | "atob"
            | "btoa"
            | "URL"
            | "URLSearchParams"
            | "TextEncoder"
            | "TextDecoder"
            | "AbortController"
            | "AbortSignal"
            | "Event"
            | "EventTarget"
            | "Blob"
            | "FormData"
            | "Headers"
            | "Request"
            | "Response"
            | "fetch"
            | "performance"
            | "window"
            | "document"
            | "navigator"
            | "location"
            | "localStorage"
            | "sessionStorage"
            | "NaN"
            | "Infinity"
            | "parseInt"
            | "parseFloat"
            | "isNaN"
            | "isFinite"
            | "setTimeout"
            | "clearTimeout"
            | "setInterval"
            | "clearInterval"
    )
}
