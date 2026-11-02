use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::unit::CancelToken;

use super::graph::ModuleGraph;

// Problem: the project driver checks one graph layer at a time and waits at the end of each,
// so a unit that is ready the moment its own imports finish still waits for the slowest unit
// of its layer.
// Picked: a ready queue over the components of the graph, so a cycle is one unit, as in E3.
// Every unit counts the units it waits for, and the thread that finishes the last of them
// starts it. A unit reads only the results of the units it waits for, and those are
// finished before it starts, so what it computes does not depend on how the threads
// interleave. A unit that panics is recorded as such and its dependents still run, so one
// bad file does not silence the files that import it; the caller turns the record into its
// own incomplete result. Cancellation is checked as a unit starts, never inside it.
// Cost: independent units overlap in any order, so the order they finish in means nothing
// and nothing may be derived from it. Results come back indexed by unit, and whatever is
// built from them (a report, the project's export tables) has to be assembled in that
// order afterwards.

#[derive(Debug)]
pub enum UnitOutcome<R> {
    Done(R),
    /// The work panicked. Its dependents still ran.
    Panicked,
    /// The token was cancelled before the unit started, so its work never ran.
    Cancelled,
}

/// The units that can never start because they wait on each other, or on themselves.
#[derive(Debug, PartialEq, Eq)]
pub struct Cyclic(pub Vec<usize>);

/// The results of the units that have finished. A unit's work is called only after every
/// unit it waits for is in here, so `get` of one of those never returns None.
pub struct Finished<'a, R> {
    slots: &'a [OnceLock<UnitOutcome<R>>],
}

impl<R> Finished<'_, R> {
    pub fn get(&self, unit: usize) -> Option<&UnitOutcome<R>> {
        self.slots.get(unit)?.get()
    }
}

/// What each strongly connected component of `graph` waits for: the other components its
/// members import, and the files they load dynamically that `ModuleGraph::ordered_dynamic`
/// lists, sorted and without duplicates. The index of a unit is its position in
/// `ModuleGraph::sccs`, which lists dependencies first. A unit waits only for lower indices
/// unless it waits on a dynamic import, which can point either way; none of them closes a
/// cycle, so the units still form a DAG.
pub fn unit_dependencies(graph: &ModuleGraph) -> Vec<Vec<usize>> {
    graph
        .sccs()
        .iter()
        .enumerate()
        .map(|(unit, members)| {
            let mut waits: Vec<usize> = members
                .iter()
                .flat_map(|member| {
                    graph
                        .dependencies(*member)
                        .iter()
                        .chain(graph.ordered_dynamic(*member))
                })
                .map(|dependency| graph.scc_of(*dependency))
                .filter(|component| *component != unit)
                .collect();
            waits.sort_unstable();
            waits.dedup();
            waits
        })
        .collect()
}

struct Run<'a, R, W> {
    slots: Vec<OnceLock<UnitOutcome<R>>>,
    waiting: Vec<AtomicUsize>,
    dependents: Vec<Vec<usize>>,
    cancel: Option<&'a CancelToken>,
    work: W,
}

fn start<'s, R, W>(scope: &rayon::Scope<'s>, run: &'s Run<'s, R, W>, unit: usize)
where
    R: Send + Sync + 's,
    W: Fn(usize, &Finished<'_, R>) -> R + Sync + 's,
{
    scope.spawn(move |scope| {
        let outcome = if run.cancel.is_some_and(CancelToken::is_cancelled) {
            UnitOutcome::Cancelled
        } else {
            let finished = Finished { slots: &run.slots };
            match catch_unwind(AssertUnwindSafe(|| (run.work)(unit, &finished))) {
                Ok(result) => UnitOutcome::Done(result),
                Err(_) => UnitOutcome::Panicked,
            }
        };
        // A unit starts once, so the slot is empty here.
        let _ = run.slots[unit].set(outcome);
        for &next in &run.dependents[unit] {
            if run.waiting[next].fetch_sub(1, Ordering::AcqRel) == 1 {
                start(scope, run, next);
            }
        }
    });
}

fn launch<'s, R, W>(scope: &rayon::Scope<'s>, run: &'s Run<'s, R, W>, roots: &[usize])
where
    R: Send + Sync + 's,
    W: Fn(usize, &Finished<'_, R>) -> R + Sync + 's,
{
    for unit in roots {
        start(scope, run, *unit);
    }
}

/// Runs `work` once for every unit, each after all the units in `dependencies[unit]` have
/// finished, on `pool` (rayon's global pool when None). Independent units overlap.
/// `work` receives the unit and the results finished so far, and must read only the units
/// it waits for. The outcomes come back indexed by unit.
///
/// Every index in `dependencies` must be below its length. Listing a dependency twice is
/// the same as listing it once. Units that wait on each other are refused before anything
/// runs, since they would never start.
pub fn run_units<R, W>(
    pool: Option<&rayon::ThreadPool>,
    dependencies: &[Vec<usize>],
    cancel: Option<&CancelToken>,
    work: W,
) -> Result<Vec<UnitOutcome<R>>, Cyclic>
where
    R: Send + Sync,
    W: Fn(usize, &Finished<'_, R>) -> R + Sync,
{
    let count = dependencies.len();
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); count];
    let mut needed = vec![0usize; count];
    for (unit, waits) in dependencies.iter().enumerate() {
        let mut waits = waits.clone();
        waits.sort_unstable();
        waits.dedup();
        needed[unit] = waits.len();
        for dependency in waits {
            dependents[dependency].push(unit);
        }
    }

    let mut remaining = needed.clone();
    let mut ready: Vec<usize> = (0..count).filter(|unit| remaining[*unit] == 0).collect();
    let mut reached = 0;
    while let Some(unit) = ready.pop() {
        reached += 1;
        for &next in &dependents[unit] {
            remaining[next] -= 1;
            if remaining[next] == 0 {
                ready.push(next);
            }
        }
    }
    if reached < count {
        return Err(Cyclic(
            (0..count).filter(|unit| remaining[*unit] > 0).collect(),
        ));
    }

    let roots: Vec<usize> = (0..count).filter(|unit| needed[*unit] == 0).collect();
    let run = Run {
        slots: (0..count).map(|_| OnceLock::new()).collect(),
        waiting: needed.iter().map(|n| AtomicUsize::new(*n)).collect(),
        dependents,
        cancel,
        work,
    };
    match pool {
        Some(pool) => pool.scope(|scope| launch(scope, &run, &roots)),
        None => rayon::scope(|scope| launch(scope, &run, &roots)),
    }

    // Every unit has finished once the scope returns, so no slot is empty; Cancelled is the
    // safe reading if one ever were.
    Ok(run
        .slots
        .into_iter()
        .map(|slot| slot.into_inner().unwrap_or(UnitOutcome::Cancelled))
        .collect())
}
