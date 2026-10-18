use iai_callgrind::{library_benchmark, library_benchmark_group, main};
use std::hint::black_box;
use std::path::PathBuf;
use tsox::{ModuleGraph, ModuleResolver, check_project};

#[path = "support/project_fixtures.rs"]
mod project_fixtures;

use project_fixtures::Project;

// Each argument is the entry paths of a project written before the benchmark runs. The
// projects are persistent, so nothing is deleted inside the measured function.

fn layered_entries() -> Vec<PathBuf> {
    Project::persistent("layered")
        .layered(10, 6, 3)
        .entries
        .clone()
}

fn ring_entries() -> Vec<PathBuf> {
    Project::persistent("ring").ring(20).entries.clone()
}

fn independent_entries() -> Vec<PathBuf> {
    Project::persistent("independent")
        .independent(20, 20)
        .entries
        .clone()
}

// Resolution and discovery only. Reading files, parsing each for its imports and
// resolving them is single threaded, so the instruction count is stable.
#[library_benchmark]
#[bench::layered_61_files(layered_entries())]
#[bench::ring_20_files(ring_entries())]
fn build_graph(entries: Vec<PathBuf>) {
    let resolver = ModuleResolver::new();
    let graph = ModuleGraph::build(black_box(&entries), &resolver);
    assert!(graph.is_ok(), "benchmark graph failed to build");
    let _ = black_box(graph);
}

// The graph build plus the check, on a pool of one thread. A larger pool would make the
// instruction count depend on scheduling; one thread keeps it repeatable, at the price
// of also counting the creation of the pool.
#[library_benchmark]
#[bench::layered_61_files(layered_entries())]
#[bench::independent_20_files(independent_entries())]
fn check_project_single_thread(entries: Vec<PathBuf>) {
    let resolver = ModuleResolver::new();
    let graph =
        ModuleGraph::build(black_box(&entries), &resolver).expect("benchmark graph should build");
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .expect("thread pool should build");

    let report = pool.install(|| check_project(black_box(&graph)));

    assert_eq!(report.failure_count(), 0);
    let _ = black_box(report);
}

library_benchmark_group!(
    name = module_group;
    benchmarks = build_graph, check_project_single_thread
);

main!(library_benchmark_groups = module_group);
