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
pub const BIG_STACK_BYTES: usize = 64 * 1024 * 1024;

/// Every numeric limit a walk of the checker may reach, in one place. A walk that reaches one
/// stops and records its `CutReason`, so a cut is never silent. The stack limit is not a
/// number here: it comes from the thread (`set_thread_stack`). Changing a limit is a change
/// to this table, and `Budget::limit_of` is how a test or `--stats` asks what a reason's
/// limit is.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Budget {
    /// Levels one substitution walk may nest (`CutReason::InstantiationDepth`).
    pub instantiation_depth: usize,
    /// Substitution steps one statement may take, as tsc's `instantiationCount`
    /// (`CutReason::InstantiationCount`, TS2589).
    pub instantiations: u64,
    /// Times one recursive generic application may instantiate itself
    /// (`CutReason::RecursiveApplication`).
    pub recursive_applications: usize,
    /// Nesting of `(`, `[` and `{` a file may have before it is refused unparsed.
    pub bracket_depth: usize,
}

pub const BUDGET: Budget = Budget {
    instantiation_depth: 100,
    instantiations: 5_000_000,
    recursive_applications: 100,
    bracket_depth: 4_000,
};

impl Budget {
    /// The number behind a cut reason, or `None` for one the thread's stack decides.
    pub fn limit_of(&self, reason: CutReason) -> Option<u64> {
        match reason {
            CutReason::InstantiationDepth => Some(self.instantiation_depth as u64),
            CutReason::InstantiationCount => Some(self.instantiations),
            CutReason::RecursiveApplication => Some(self.recursive_applications as u64),
            CutReason::StackExhausted | CutReason::AliasBodyNesting => None,
        }
    }
}

/// How deeply a file may nest `(`, `[` and `{` before it is refused without being parsed.
///
/// The stack guard cannot reach the parser's recursion, and in a debug build on a 64 MB
/// stack the worst construct measured, nested objects, aborts at about 20,000 levels. This
/// leaves a factor of five. Real code nests a few dozen levels, so the refusal is for a
/// file built to be deep, and it is made before the parse so that nothing can abort first.
pub const MAX_BRACKET_DEPTH: usize = BUDGET.bracket_depth;

thread_local! {
    // How many times a walk that has nowhere to report stopped at a limit, by reason (see
    // `note_cut`). Indexed by `CutReason as usize`.
    static CUTS: Cell<[u32; CutReason::COUNT]> = const { Cell::new([0; CutReason::COUNT]) };
    // Substitution steps taken since the last `reset_instantiations`.
    static INSTANTIATIONS: Cell<u64> = const { Cell::new(0) };
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
pub fn set_thread_stack(total_bytes: usize) {
    let usable = total_bytes - margin(total_bytes);
    FLOOR.with(|floor| floor.set(stack_position().saturating_sub(usable)));
}

/// Why a walk stopped at a limit. Every limit names itself, so a cut is never silent and a test
/// or a `--stats` line can say which budget ran out.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CutReason {
    /// The thread's stack budget was used up (`exhausted`).
    StackExhausted,
    /// A substitution walk went past `MAX_INSTANTIATION_DEPTH` levels.
    InstantiationDepth,
    /// A unit took more than `MAX_INSTANTIATIONS` substitution steps.
    InstantiationCount,
    /// A recursive generic application instantiated itself too many times over.
    RecursiveApplication,
    /// A type alias body nested too deep for the reference scan.
    AliasBodyNesting,
}

impl CutReason {
    pub const COUNT: usize = 5;
    pub const ALL: [CutReason; Self::COUNT] = [
        CutReason::StackExhausted,
        CutReason::InstantiationDepth,
        CutReason::InstantiationCount,
        CutReason::RecursiveApplication,
        CutReason::AliasBodyNesting,
    ];

    pub fn name(self) -> &'static str {
        match self {
            CutReason::StackExhausted => "stack exhausted",
            CutReason::InstantiationDepth => "instantiation depth",
            CutReason::InstantiationCount => "instantiation count",
            CutReason::RecursiveApplication => "recursive application",
            CutReason::AliasBodyNesting => "alias body nesting",
        }
    }
}

/// Records that a walk without a context to report to (type substitution) stopped at a limit and
/// returned the error type. The unit reads it once with `take_cut` and says so with TSR9102.
pub fn note_cut(reason: CutReason) {
    CUTS.with(|cuts| {
        let mut counts = cuts.get();
        counts[reason as usize] = counts[reason as usize].saturating_add(1);
        cuts.set(counts);
    });
}

/// Whether a walk was cut since the last call, and clears the record.
pub fn take_cut() -> bool {
    !take_cut_reasons().is_empty()
}

/// The reasons walks were cut since the last call, with how many times each, and clears the
/// record. Empty when nothing was cut.
pub fn take_cut_reasons() -> Vec<(CutReason, u32)> {
    let counts = CUTS.with(|cuts| cuts.replace([0; CutReason::COUNT]));
    CutReason::ALL
        .into_iter()
        .filter(|reason| counts[*reason as usize] > 0)
        .map(|reason| (reason, counts[reason as usize]))
        .collect()
}

/// True when the calling thread has used its budget and must not recurse further.
/// Always false on a thread that did not call `set_thread_stack`.
pub fn exhausted() -> bool {
    FLOOR.with(|floor| {
        let floor = floor.get();
        floor != 0 && stack_position() < floor
    })
}

/// Runs `work` on a fresh thread with a `BIG_STACK_BYTES` stack and a guard budget, and
/// returns what it returned. A panic in `work` is raised again here. If the thread cannot
/// be created, `work` runs on the calling thread, whose stack is whatever it is.
pub fn on_big_stack<R: Send>(work: impl FnOnce() -> R + Send) -> R {
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
    fn a_cut_names_its_reason_and_take_clears_it() {
        let _ = take_cut_reasons();
        note_cut(CutReason::InstantiationDepth);
        note_cut(CutReason::InstantiationDepth);
        note_cut(CutReason::StackExhausted);
        assert_eq!(
            take_cut_reasons(),
            vec![
                (CutReason::StackExhausted, 1),
                (CutReason::InstantiationDepth, 2)
            ]
        );
        assert!(!take_cut());
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

/// TypeScript stops a statement or expression that has instantiated this many types
/// (`instantiationCount` in checker.go) and reports TS2589.
pub const MAX_INSTANTIATIONS: u64 = BUDGET.instantiations;

/// Starts a new budget, at the top of each statement the checker visits.
pub fn reset_instantiations() {
    INSTANTIATIONS.with(|count| count.set(0));
}

/// Counts one substitution step. False once the budget is spent, after which the caller
/// returns the error type and `note_cut` records it.
pub fn count_instantiation() -> bool {
    INSTANTIATIONS.with(|count| {
        let next = count.get() + 1;
        count.set(next);
        next <= MAX_INSTANTIATIONS
    })
}

#[cfg(test)]
pub(crate) fn instantiations_taken() -> u64 {
    INSTANTIATIONS.with(Cell::get)
}

#[cfg(test)]
mod instantiation_budget_tests {
    use super::*;

    #[test]
    fn budget_runs_out_after_the_limit_and_reset_restores_it() {
        reset_instantiations();
        INSTANTIATIONS.with(|count| count.set(MAX_INSTANTIATIONS - 1));
        assert!(count_instantiation());
        assert!(!count_instantiation());
        reset_instantiations();
        assert_eq!(instantiations_taken(), 0);
        assert!(count_instantiation());
    }
}

#[cfg(test)]
mod budget_tests {
    use super::*;

    #[test]
    fn every_reason_that_has_a_number_has_a_positive_one() {
        for reason in CutReason::ALL {
            if let Some(limit) = BUDGET.limit_of(reason) {
                assert!(limit > 0, "{}", reason.name());
            }
        }
        assert_eq!(BUDGET.limit_of(CutReason::StackExhausted), None);
    }
}
