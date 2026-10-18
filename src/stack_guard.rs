use std::cell::Cell;
use std::sync::Mutex;

// Problem: the checker walks expressions and type declarations by recursion, so an input
// nested deeply enough overflows the thread's stack. That is an abort, not a panic: the
// worker's catch_unwind cannot see it and one file loses the whole run (E0).
// Picked: a thread that is going to run checker code says how big its stack is, and the
// recursive entry points ask `exhausted()` before going one level deeper. The answer is
// by address, not by a depth count, because a level costs very different amounts in a
// debug and a release build and in an expression and a type walk, and the address is
// the one number that is right in all of them. It needs no dependency and no unsafe.
// Cost: a thread that never called `set_thread_stack` is not guarded, which is every
// caller that has not opted in. And the guard covers our own recursion only: the parser
// (oxc) recurses on nested syntax and has no limit of its own, so a parse is protected
// only by running it on a stack of known size (`on_big_stack`), not by this check.
// Now: the project workers, `ModuleGraph::build` and `with_deep_stack` set a budget; the
// two walks that overflowed in the fuzz run (infer_expression_type, TypeNamespace::resolve)
// stop with a diagnostic and an error type instead of recursing.

/// The stack the checker's own threads are given. The project worker pool uses the same
/// size, so a file discovery could parse is a file a worker can parse.
pub(crate) const BIG_STACK_BYTES: usize = 64 * 1024 * 1024;

/// How deeply a file may nest `(`, `[` and `{` before it is refused without being parsed.
///
/// The stack guard cannot reach the parser's recursion, and in a debug build on a 64 MB
/// stack the worst construct measured, nested objects, aborts at about 20,000 levels. This
/// leaves a factor of five. Real code nests a few dozen levels, so the refusal is for a
/// file built to be deep, and it is made before the parse so that nothing can abort first.
pub(crate) const MAX_BRACKET_DEPTH: usize = 4_000;

thread_local! {
    // The lowest address this thread may reach before `exhausted()` says so. Zero means
    // the thread has not set a budget and is not checked.
    static FLOOR: Cell<usize> = const { Cell::new(0) };
}

// The address of a local, which is as near the stack pointer as safe code can get. The
// stack grows down on every target this crate builds for, wasm's shadow stack included.
#[inline(never)]
fn stack_position() -> usize {
    let marker = 0u8;
    std::hint::black_box(&marker) as *const u8 as usize
}

// What is held back from the budget for the code between two checks: the parser, the
// semantic builder and the display code recurse too and are not guarded. An eighth of the
// stack, at least 256 KiB and never more than half of it.
fn margin(total_bytes: usize) -> usize {
    (total_bytes / 8).max(256 * 1024).min(total_bytes / 2)
}

/// Declares that the calling thread has a stack of `total_bytes`, starting from where it
/// stands now. Call it near the top of the thread, before any deep work.
pub(crate) fn set_thread_stack(total_bytes: usize) {
    let usable = total_bytes - margin(total_bytes);
    FLOOR.with(|floor| floor.set(stack_position().saturating_sub(usable)));
}

/// True when the calling thread has used its budget and must not recurse further.
/// Always false on a thread that did not call `set_thread_stack`.
pub(crate) fn exhausted() -> bool {
    FLOOR.with(|floor| {
        let floor = floor.get();
        floor != 0 && stack_position() < floor
    })
}

/// Runs `work` on a fresh thread with a `BIG_STACK_BYTES` stack and a guard budget, and
/// returns what it returned. A panic in `work` is raised again here. If the thread cannot
/// be created, `work` runs on the calling thread, whose stack is whatever it is.
pub(crate) fn on_big_stack<R: Send>(work: impl FnOnce() -> R + Send) -> R {
    // The closure is handed to the thread through a slot, so that if the thread cannot
    // be spawned the closure has not been consumed by the failed attempt.
    let slot = Mutex::new(Some(work));
    let take = || {
        slot.lock()
            .ok()
            .and_then(|mut guard| guard.take())
            .expect("the work is taken once")
    };
    std::thread::scope(|scope| {
        let spawned = std::thread::Builder::new()
            .stack_size(BIG_STACK_BYTES)
            .spawn_scoped(scope, || {
                set_thread_stack(BIG_STACK_BYTES);
                take()()
            });
        match spawned {
            Ok(handle) => match handle.join() {
                Ok(result) => result,
                Err(panic) => std::panic::resume_unwind(panic),
            },
            Err(_) => take()(),
        }
    })
}

/// Runs `work` on a thread with a 64 MB stack and the checker's recursion guard armed.
///
/// A caller that checks untrusted source on a thread of its own, such as an editor or a
/// server, can wrap the call in this so that a deeply nested file is answered with a
/// diagnostic and not a stack overflow. The parser's own recursion is not guarded; it
/// only gets the larger stack.
pub fn with_deep_stack<R: Send>(work: impl FnOnce() -> R + Send) -> R {
    on_big_stack(work)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A recursion that only stops because the guard says so. The frame is padded so the
    // test does not need millions of levels to use a megabyte.
    fn descend(depth: usize) -> usize {
        let padding = [depth as u8; 256];
        std::hint::black_box(&padding);
        if exhausted() {
            return depth;
        }
        descend(depth + 1).max(padding[0] as usize)
    }

    #[test]
    fn a_thread_without_a_budget_is_never_exhausted() {
        std::thread::spawn(|| assert!(!exhausted())).join().unwrap();
    }

    #[test]
    fn the_guard_stops_a_recursion_before_a_small_stack_is_gone() {
        // 2 MiB is the default for a spawned thread, the smallest stack the checker meets.
        let total = 2 * 1024 * 1024;
        let depth = std::thread::Builder::new()
            .stack_size(total)
            .spawn(move || {
                set_thread_stack(total);
                descend(0)
            })
            .unwrap()
            .join()
            .expect("the guard stopped the recursion before it overflowed");
        assert!(depth > 100, "it should have gone some way down: {depth}");
    }

    #[test]
    fn the_budget_is_per_thread() {
        let total = 2 * 1024 * 1024;
        std::thread::Builder::new()
            .stack_size(total)
            .spawn(move || set_thread_stack(total))
            .unwrap()
            .join()
            .unwrap();
        // This thread set nothing, so another thread's budget does not reach it.
        assert!(!exhausted());
    }

    #[test]
    fn on_big_stack_returns_the_result_and_arms_the_guard() {
        let (value, armed) = on_big_stack(|| {
            let before = exhausted();
            (41 + 1, !before && descend(0) > 1000)
        });
        assert_eq!(value, 42);
        assert!(armed);
    }

    #[test]
    fn on_big_stack_raises_a_panic_again() {
        let caught = std::panic::catch_unwind(|| on_big_stack(|| panic!("boom")));
        assert!(caught.is_err());
    }
}
