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
pub fn resolve_identifier_type(
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
        // `arguments` is the implicit object of every non-arrow function; it is not declared
        // by any library file. Typed `any` rather than IArguments: indexing and `.length` work.
        if ident.name == "arguments" {
            return ctx.arena.any();
        }
        // Oxc leaves a value read of an `import type` name without a symbol. The name is
        // declared and has a value side, but the import erased it: TS1361, not 2304/2693.
        if ctx.type_only_value_names.contains(ident.name.as_str()) {
            ctx.error(
                crate::diagnostic_messages::messages::import_type_only_value(&ident.name),
                ident.span(),
            );
            return ctx.arena.error();
        }
        if let Some(&global_type) = ctx.global_values.get(ident.name.as_str()) {
            return global_type;
        }
        // Standalone checks have no project library providers, so retain the historical
        // ambient-global fallback below for compatibility. Configured projects instead
        // obtain these names from the selected standard-library declarations.
        // Keep the legacy `any` fallback for standalone checks, which have no project
        // library provider. In a configured project, selected .d.ts declarations are the
        // source of truth: an unselected global such as `document` under `lib: ["ES5"]`
        // must stay unresolved instead of silently becoming `any`.
        let use_legacy_ambient_fallback = ctx
            .imports
            .is_none_or(|source| !source.requires_registered_global_values());
        if use_legacy_ambient_fallback && is_ambient_global(&ident.name) {
            return ctx.arena.any();
        }
        // A project global whose declaration could not be carried over: it exists, but its
        // type is not known.
        if ctx.namespace.is_unmodelled_global(&ident.name)
            && !ctx.namespace.is_unmodelled_type_only(&ident.name)
        {
            return ctx.arena.any();
        }
        // Problem: `namespace A.B { export function f() {} }` then `namespace A { function g() {
        // B.f(); } }`: oxc keeps one scope per block, so `B` (declared in the other block of
        // `A`) has no symbol here, and tsc finds it as a member of the merged namespace.
        // Picked: inside a namespace block, a name that some declaration in the file binds is
        // read as `any`. Only a name declared directly in a namespace block counts: an enum
        // member or a type parameter of the same spelling is not in reach from another block.
        // Cost: the member's real type is not known here, so nothing is checked through it.
        if !ctx.namespace.scope_prefixes().is_empty()
            && scoping.symbol_ids().any(|symbol| {
                scoping.symbol_name(symbol) == ident.name.as_str()
                    && scoping
                        .scope_flags(scoping.symbol_scope_id(symbol))
                        .contains(oxc_semantic::ScopeFlags::TsModuleBlock)
            })
        {
            return ctx.arena.any();
        }
        // Problem: `Iterator` or a user's own `interface I {}` used as a value read as "Cannot
        // find name", where tsc says the name only refers to a type (TS2693).
        // Picked: a name this checker knows as a type, or one of the library types that have
        // no value, gets TS2693.
        let message = if ctx.namespace.contains(&ident.name)
            || ctx.namespace.is_unmodelled_type_only(&ident.name)
            || is_library_type_only(&ident.name)
        {
            crate::diagnostic_messages::messages::type_used_as_value(&ident.name)
        } else {
            crate::diagnostic_messages::messages::unresolved_identifier_hint(&ident.name)
                .unwrap_or_else(|| {
                    crate::diagnostic_messages::messages::unresolved_identifier(&ident.name)
                })
        };
        ctx.error(message, ident.span());
        return ctx.arena.error();
    };

    // Declarations occupy separate type and value spaces. An interface/alias has
    // no value-space side (TS2693); a class/enum/function/variable imported with
    // `import type` has a value side in its module but no local runtime binding
    // (TS1361). Resolve that distinction by SymbolId, not by spelling.
    if let Some(spaces) = ctx.symbol_capabilities.get(&symbol_id).copied() {
        if spaces.has_value && !spaces.value_available {
            ctx.error(
                crate::diagnostic_messages::messages::import_type_only_value(&ident.name),
                ident.span(),
            );
            return ctx.arena.error();
        }
        if spaces.has_type && !spaces.has_value {
            ctx.error(
                crate::diagnostic_messages::messages::type_used_as_value(&ident.name),
                ident.span(),
            );
            return ctx.arena.error();
        }
    }

    let result = match ctx.narrow.get(symbol_id) {
        Some(narrowed) => Some(narrowed),
        None => ctx.symbols.get(symbol_id),
    };
    let result = result.map(|r| super::super::expando::apply(symbol_id, r, ctx));
    if let Some(result) = result {
        // The backward flow walk, when it is on, answers the read if it can; otherwise this is
        // the forward overlay's answer, noted for the comparison in shadow mode.
        if let Some(mut flow) = ctx.shadow.take() {
            let walked = flow.on_read(
                ctx,
                scoping,
                ident.node_id.get().index() as u32,
                symbol_id,
                &ident.name,
                ident.span(),
                result,
            );
            ctx.shadow = Some(flow);
            if let Some(walked) = walked {
                return walked;
            }
        }
        return result;
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
pub(in crate::bridge) fn is_library_type_only(name: &str) -> bool {
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

// Legacy fallback for standalone checks, which have no project library providers. A
// configured project must resolve these names from the selected `.d.ts` declarations or
// report them as unresolved; otherwise an unselected DOM/runtime global would silently be
// accepted as `any`.
pub(in crate::bridge) fn is_ambient_global(name: &str) -> bool {
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
