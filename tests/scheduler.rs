#![cfg(feature = "module-resolution")]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tsox::{
    CancelToken, Cyclic, ModuleGraph, ModuleResolver, UnitOutcome, run_units, unit_dependencies,
};

// Problem: the scheduler starts a unit the moment its imports finish, so units overlap in any
// order, and nothing in the checker may depend on that order. A unit that starts early, runs
// twice, or reads a result that is not there yet would show up as a wrong report only on some
// machines.
// Picked: random acyclic graphs with shuffled labels, so an index says nothing about depth,
// run on pools of one, two and eight threads with a different delay per unit and seed. Each
// unit records when it started and finished on one shared clock. The tests then require every
// unit to run exactly once, to start after all of its dependencies finished, and to produce
// the value a plain sequential walk produces. The cases the driver leans on are pinned apart
// from that: a panic, a cancel, and units that wait on each other.
// Now: all of them hold. The scheduler is not yet what check_project uses.

struct Xorshift(u64);

impl Xorshift {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }
}

fn pool(threads: usize) -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap()
}

// A random acyclic graph. The labels are shuffled, so a unit's index does not tell how deep it
// sits and a ready unit is often not the lowest index.
fn random_dag(rng: &mut Xorshift) -> Vec<Vec<usize>> {
    let count = 1 + rng.below(40);
    let density = 1 + rng.below(6);
    let mut order: Vec<usize> = (0..count).collect();
    for at in (1..count).rev() {
        order.swap(at, rng.below(at + 1));
    }
    let mut dependencies = vec![Vec::new(); count];
    for later in 0..count {
        for earlier in 0..later {
            if rng.below(count.max(2)) < density {
                dependencies[order[later]].push(order[earlier]);
            }
        }
    }
    dependencies
}

fn expected(unit: usize, dependencies: &[Vec<usize>], memo: &mut Vec<Option<u64>>) -> u64 {
    if let Some(value) = memo[unit] {
        return value;
    }
    let mut value = unit as u64 * 31 + 7;
    for dependency in &dependencies[unit] {
        value = value.wrapping_add(expected(*dependency, dependencies, memo).wrapping_mul(31));
    }
    memo[unit] = Some(value);
    value
}

#[test]
fn every_unit_runs_once_after_its_dependencies_on_random_graphs() {
    for seed in 1..=120u64 {
        let mut rng = Xorshift(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let dependencies = random_dag(&mut rng);
        let count = dependencies.len();
        let mut memo = vec![None; count];
        let want: Vec<u64> = (0..count)
            .map(|unit| expected(unit, &dependencies, &mut memo))
            .collect();

        for threads in [1, 2, 8] {
            let clock = AtomicUsize::new(0);
            let runs: Vec<AtomicUsize> = (0..count).map(|_| AtomicUsize::new(0)).collect();
            let started: Vec<AtomicUsize> =
                (0..count).map(|_| AtomicUsize::new(usize::MAX)).collect();
            let ended: Vec<AtomicUsize> =
                (0..count).map(|_| AtomicUsize::new(usize::MAX)).collect();

            let outcomes = run_units::<u64, _>(
                Some(&pool(threads)),
                &dependencies,
                None,
                |unit, finished| {
                    runs[unit].fetch_add(1, Ordering::SeqCst);
                    started[unit].store(clock.fetch_add(1, Ordering::SeqCst), Ordering::SeqCst);
                    let micros = (unit as u64 * 37 + seed * 11 + threads as u64) % 300;
                    std::thread::sleep(Duration::from_micros(micros));
                    let mut value = unit as u64 * 31 + 7;
                    for dependency in &dependencies[unit] {
                        match finished.get(*dependency) {
                            Some(UnitOutcome::Done(done)) => {
                                value = value.wrapping_add(done.wrapping_mul(31));
                            }
                            other => panic!("unit {unit} started before {dependency}: {other:?}"),
                        }
                    }
                    ended[unit].store(clock.fetch_add(1, Ordering::SeqCst), Ordering::SeqCst);
                    value
                },
            )
            .unwrap();

            for unit in 0..count {
                assert_eq!(
                    runs[unit].load(Ordering::SeqCst),
                    1,
                    "seed {seed} unit {unit}"
                );
                for dependency in &dependencies[unit] {
                    assert!(
                        ended[*dependency].load(Ordering::SeqCst)
                            < started[unit].load(Ordering::SeqCst),
                        "seed {seed} threads {threads}: {unit} started before {dependency} ended"
                    );
                }
                match &outcomes[unit] {
                    UnitOutcome::Done(value) => {
                        assert_eq!(*value, want[unit], "seed {seed} threads {threads}")
                    }
                    other => panic!("seed {seed} unit {unit}: {other:?}"),
                }
            }
        }
    }
}

#[test]
fn a_unit_that_panics_is_recorded_and_its_dependents_still_run() {
    // 0 <- 1 <- 2, and 3 stands alone.
    let dependencies = vec![vec![], vec![0], vec![1], vec![]];
    let outcomes = run_units(None, &dependencies, None, |unit, finished| {
        if unit == 1 {
            panic!("unit 1 fails");
        }
        if unit == 2 {
            assert!(matches!(finished.get(1), Some(UnitOutcome::Panicked)));
        }
        unit as u64
    })
    .unwrap();
    assert!(matches!(outcomes[0], UnitOutcome::Done(0)));
    assert!(matches!(outcomes[1], UnitOutcome::Panicked));
    assert!(matches!(outcomes[2], UnitOutcome::Done(2)));
    assert!(matches!(outcomes[3], UnitOutcome::Done(3)));
}

#[test]
fn a_cancel_stops_every_unit_that_has_not_started() {
    let dependencies = vec![vec![], vec![0], vec![1]];
    let token = CancelToken::new();
    let calls = AtomicUsize::new(0);
    let outcomes = run_units(Some(&pool(2)), &dependencies, Some(&token), |unit, _| {
        calls.fetch_add(1, Ordering::SeqCst);
        if unit == 0 {
            token.cancel();
        }
        unit as u64
    })
    .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(matches!(outcomes[0], UnitOutcome::Done(0)));
    assert!(matches!(outcomes[1], UnitOutcome::Cancelled));
    assert!(matches!(outcomes[2], UnitOutcome::Cancelled));
}

#[test]
fn a_token_cancelled_before_the_run_starts_no_unit() {
    let dependencies = vec![vec![], vec![0], vec![]];
    let token = CancelToken::new();
    token.cancel();
    let calls = AtomicUsize::new(0);
    let outcomes = run_units(None, &dependencies, Some(&token), |unit, _| {
        calls.fetch_add(1, Ordering::SeqCst);
        unit as u64
    })
    .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(
        outcomes
            .iter()
            .all(|outcome| matches!(outcome, UnitOutcome::Cancelled))
    );
}

#[test]
fn units_that_wait_on_each_other_are_refused_before_any_work_runs() {
    // 0 and 1 wait on each other, 2 is free, 3 waits on itself, 4 waits on 1.
    let dependencies = vec![vec![1], vec![0], vec![], vec![3], vec![1]];
    let calls = AtomicUsize::new(0);
    let refused = run_units(None, &dependencies, None, |unit, _| {
        calls.fetch_add(1, Ordering::SeqCst);
        unit as u64
    })
    .unwrap_err();
    assert_eq!(refused, Cyclic(vec![0, 1, 3, 4]));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn a_dependency_listed_twice_counts_once() {
    let dependencies = vec![vec![], vec![0, 0, 0]];
    let outcomes = run_units(None, &dependencies, None, |unit, _| unit as u64).unwrap();
    assert!(matches!(outcomes[1], UnitOutcome::Done(1)));
}

#[test]
fn an_empty_project_has_no_units() {
    let outcomes = run_units(None, &[], None, |unit, _| unit as u64).unwrap();
    assert!(outcomes.is_empty());
}

fn graph_of(test: &str, files: &[(&str, &str)]) -> ModuleGraph {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("scheduler-{test}"));
    let _ = fs::remove_dir_all(&root);
    let mut paths: Vec<PathBuf> = Vec::new();
    for (name, source) in files {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, source).unwrap();
        paths.push(path);
    }
    ModuleGraph::build(&paths, &ModuleResolver::new())
        .expect("the project builds")
        .with_root(root)
}

#[test]
fn a_cycle_of_files_is_one_unit_that_its_importers_wait_for() {
    let graph = graph_of(
        "cycle-unit",
        &[
            ("a.ts", "import { b } from './b';\nexport const a = b;\n"),
            ("b.ts", "import { c } from './c';\nexport const b = 1;\n"),
            ("c.ts", "import { b } from './b';\nexport const c = 2;\n"),
            ("d.ts", "import { a } from './a';\nexport const d = a;\n"),
        ],
    );
    let dependencies = unit_dependencies(&graph);
    assert_eq!(dependencies.len(), 3, "{dependencies:?}");

    let names: Vec<Vec<String>> = graph
        .sccs()
        .iter()
        .map(|members| {
            let mut names: Vec<String> = members
                .iter()
                .map(|id| {
                    let path = graph.files().path(*id).unwrap();
                    path.file_name().unwrap().to_string_lossy().into_owned()
                })
                .collect();
            names.sort();
            names
        })
        .collect();
    let at = |wanted: &[&str]| {
        names
            .iter()
            .position(|unit| unit.iter().map(String::as_str).eq(wanted.iter().copied()))
            .unwrap_or_else(|| panic!("no unit {wanted:?} in {names:?}"))
    };
    let (cycle, a, d) = (at(&["b.ts", "c.ts"]), at(&["a.ts"]), at(&["d.ts"]));

    assert_eq!(dependencies[cycle], Vec::<usize>::new());
    assert_eq!(dependencies[a], vec![cycle]);
    assert_eq!(dependencies[d], vec![a]);
    for (unit, waits) in dependencies.iter().enumerate() {
        assert!(waits.iter().all(|wait| *wait < unit), "{dependencies:?}");
    }

    let order = AtomicUsize::new(0);
    let finished_at: Vec<AtomicUsize> = (0..3).map(|_| AtomicUsize::new(usize::MAX)).collect();
    run_units(None, &dependencies, None, |unit, _| {
        finished_at[unit].store(order.fetch_add(1, Ordering::SeqCst), Ordering::SeqCst);
        unit as u64
    })
    .unwrap();
    assert!(finished_at[cycle].load(Ordering::SeqCst) < finished_at[a].load(Ordering::SeqCst));
    assert!(finished_at[a].load(Ordering::SeqCst) < finished_at[d].load(Ordering::SeqCst));
}
