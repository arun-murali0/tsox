// Owns: narrowing by `x instanceof C`, including the library classes.

use super::*;

// `pet instanceof Dog`: keeps the members that are instances of the class, using the
// checker's own assignability, so a subclass counts as its parent. A member that is a
// supertype of the class (declared `Animal`, tested against `Dog`) becomes the class in
// the true branch. Types are structural here, as in TypeScript, so two classes with
// identical shapes cannot be told apart by instanceof, in tsc either.
//
// `any` and `unknown` become the class in the true branch and stay as they are in the
// false one. The error type and a generic parameter are left alone on both sides,
// because narrowing them would turn a gap in this checker's model into errors on valid
// code. The right-hand side must resolve to a declared type; anything else (a
// variable, a call, an unknown name) narrows nothing.
pub(super) fn narrow_by_instanceof(
    bin: &BinaryExpression,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> (NarrowState, NarrowState) {
    let Expression::Identifier(value) = &bin.left else {
        return empty_pair();
    };
    let Expression::Identifier(class) = &bin.right else {
        return empty_pair();
    };
    let Some(symbol_id) = resolve_symbol_id(value, scoping) else {
        return empty_pair();
    };
    let resolution = ctx.namespace.resolve(&class.name, &mut ctx.arena);
    let Resolution::Resolved(target) = resolution else {
        // A name the file binds itself (a variable holding a constructor) says nothing
        // about the type; one it does not bind may be a standard library class.
        if resolve_symbol_id(class, scoping).is_some() {
            return empty_pair();
        }
        return narrow_by_lib_instanceof(&class.name, symbol_id, ctx);
    };
    let Some(current) = current_type(ctx, symbol_id) else {
        return empty_pair();
    };

    let members = union_members(&ctx.arena, current).unwrap_or_else(|| vec![current]);
    let mut true_members = Vec::with_capacity(members.len());
    let mut false_members = Vec::with_capacity(members.len());
    for member in members {
        let (open_ended, unknowable) = {
            let ty = ctx.arena.get(member);
            (
                matches!(ty, Type::Any | Type::Unknown),
                matches!(ty, Type::Error | Type::GenericParameter(..)),
            )
        };
        if unknowable {
            true_members.push(member);
            false_members.push(member);
        } else if open_ended {
            true_members.push(target);
            false_members.push(member);
        } else if ctx.semantic().is_assignable(member, target) {
            true_members.push(member);
        } else {
            false_members.push(member);
            if ctx.semantic().is_assignable(target, member) {
                true_members.push(target);
            }
        }
    }
    let true_type = ctx.arena.alloc_union(true_members);
    let false_type = ctx.arena.alloc_union(false_members);
    (
        NarrowState::from_single(symbol_id, true_type),
        NarrowState::from_single(symbol_id, false_type),
    )
}

// How `instanceof` reads a standard library class the file does not declare. `Promise`
// and `Array` have types here, so their instances can be told apart; every other class
// (`Date`, `Error`, `Map`, ...) has no type and reads as `any`, like a `new Date()`.
#[derive(Clone, Copy)]
pub(super) enum LibClass {
    Promise,
    Array,
    Unmodelled,
}

pub(super) fn lib_class(name: &str) -> Option<LibClass> {
    match name {
        "Promise" => Some(LibClass::Promise),
        "Array" => Some(LibClass::Array),
        _ if is_lib_global_type(name) => Some(LibClass::Unmodelled),
        _ => None,
    }
}

// `value instanceof Date` where `Date` comes from the standard library. `unknown` and
// `any` become the class in the true branch, which for a class without a type is `any`,
// so `val.getTime()` is not an error, and stay as they are in the false one.
//
// For `Promise` and `Array` a member is kept in the true branch if it is one and in the
// false branch if it is not, so `fn instanceof Promise` leaves the function member of
// `F | Promise<T>` behind. A true branch with nothing left keeps the type it had, as a
// member this checker cannot place is not worth a `never`. Any other class narrows only
// `any` and `unknown`; a member it cannot tell apart stays on both sides.
pub(super) fn narrow_by_lib_instanceof(
    class: &str,
    symbol_id: SymbolId,
    ctx: &mut CheckContext<'_, '_>,
) -> (NarrowState, NarrowState) {
    let Some(kind) = lib_class(class) else {
        return empty_pair();
    };
    let Some(current) = current_type(ctx, symbol_id) else {
        return empty_pair();
    };

    let any = ctx.arena.any();
    let target = match kind {
        LibClass::Promise => ctx.arena.alloc_promise(any),
        LibClass::Array => ctx.arena.alloc(Type::Array(any)),
        LibClass::Unmodelled => any,
    };
    let members = union_members(&ctx.arena, current).unwrap_or_else(|| vec![current]);
    let mut true_members = Vec::with_capacity(members.len());
    let mut false_members = Vec::with_capacity(members.len());
    for member in members {
        let ty = ctx.arena.get(member);
        let open_ended = matches!(ty, Type::Any | Type::Unknown);
        let unknowable = matches!(ty, Type::Error | Type::GenericParameter(..));
        let instance = match kind {
            LibClass::Promise => ctx.arena.is_promise(member),
            LibClass::Array => matches!(ty, Type::Array(_)),
            LibClass::Unmodelled => false,
        };
        if open_ended {
            true_members.push(target);
            false_members.push(member);
        } else if unknowable || matches!(kind, LibClass::Unmodelled) {
            true_members.push(member);
            false_members.push(member);
        } else if instance {
            true_members.push(member);
        } else {
            false_members.push(member);
        }
    }
    let true_type = if true_members.is_empty() {
        current
    } else {
        ctx.arena.alloc_union(true_members)
    };
    let false_type = ctx.arena.alloc_union(false_members);
    (
        NarrowState::from_single(symbol_id, true_type),
        NarrowState::from_single(symbol_id, false_type),
    )
}
