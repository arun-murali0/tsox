use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use std::time::Duration;
use tsox::{ModuleGraph, ModuleResolver, check_project};

#[path = "support/project_fixtures.rs"]
mod project_fixtures;

use project_fixtures::Project;

const LAYERS: usize = 20;
const WIDTH: usize = 10;
const FAN_OUT: usize = 3;
const INDEPENDENT_FILES: usize = 200;
const FUNCTIONS_PER_FILE: usize = 20;
const RING_SIZE: usize = 50;
const CHAIN_LENGTH: usize = 100;

fn layered() -> Project {
    Project::new("layered").layered(LAYERS, WIDTH, FAN_OUT)
}

fn independent() -> Project {
    Project::new("independent").independent(INDEPENDENT_FILES, FUNCTIONS_PER_FILE)
}

fn build(project: &Project) -> ModuleGraph {
    ModuleGraph::build(&project.entries, &ModuleResolver::new())
        .expect("benchmark graph should build")
}

fn bench_build(c: &mut Criterion) {
    let project = layered();

    // A new resolver per run, so its directory and package.json caches start cold, which
    // is what a one-shot CLI invocation pays.
    c.bench_function("module_graph/build_layered_201_files", |b| {
        b.iter(|| {
            let resolver = ModuleResolver::new();
            let graph = ModuleGraph::build(black_box(&project.entries), &resolver)
                .expect("benchmark graph should build");
            assert_eq!(graph.len(), LAYERS * WIDTH + 1);
            black_box(graph)
        })
    });

    // One resolver for every run, so its caches are warm after the first. This is what a
    // long-lived process (an editor, a watch mode) pays when it rebuilds the graph, and
    // the gap to the cold run is the cost of resolution itself.
    let resolver = ModuleResolver::new();
    c.bench_function("module_graph/build_layered_201_files_warm_resolver", |b| {
        b.iter(|| {
            let graph = ModuleGraph::build(black_box(&project.entries), &resolver)
                .expect("benchmark graph should build");
            black_box(graph)
        })
    });

    let ring = Project::new("ring-build").ring(RING_SIZE);
    c.bench_function("module_graph/build_ring_50_files", |b| {
        b.iter(|| {
            let resolver = ModuleResolver::new();
            let graph = ModuleGraph::build(black_box(&ring.entries), &resolver)
                .expect("benchmark graph should build");
            assert_eq!(graph.len(), RING_SIZE);
            black_box(graph)
        })
    });
}

fn bench_graph_queries(c: &mut Criterion) {
    let project = layered();
    let graph = build(&project);

    c.bench_function("module_graph/layers_layered_201_files", |b| {
        b.iter(|| black_box(graph.layers()))
    });

    c.bench_function("module_graph/cycles_layered_201_files", |b| {
        b.iter(|| black_box(graph.cycles()))
    });

    // Nothing was touched, so this is the cost of asking "did anything change" about a
    // project where the answer is no: one stat per file.
    c.bench_function("module_graph/changed_files_untouched_201_files", |b| {
        b.iter(|| black_box(graph.changed_files()))
    });

    // The graph is as it was built, then one file gets different bytes. That file's
    // length no longer matches, so it is read and hashed, which is the slow path of the
    // file guard. The other 200 still cost one stat each.
    let mut edited = layered();
    let edited_graph = build(&edited);
    edited.write("l0_0.ts", "export const v0_0 = 12345678;\n", false);
    c.bench_function("module_graph/changed_files_one_edited_201_files", |b| {
        b.iter(|| {
            let changed = edited_graph.changed_files();
            assert_eq!(changed.len(), 1);
            black_box(changed)
        })
    });

    // A single import cycle holding every file, which is the hard case for the strongly
    // connected components pass and for layering.
    let ring = Project::new("ring-queries").ring(RING_SIZE);
    let ring_graph = build(&ring);
    c.bench_function("module_graph/cycles_ring_50_files", |b| {
        b.iter(|| {
            let cycles = ring_graph.cycles();
            assert_eq!(cycles.len(), 1);
            black_box(cycles)
        })
    });
    c.bench_function("module_graph/layers_ring_50_files", |b| {
        b.iter(|| black_box(ring_graph.layers()))
    });
}

fn bench_check_project(c: &mut Criterion) {
    let independent = independent();
    let independent_graph = build(&independent);
    c.bench_function("check_project/independent_200_files", |b| {
        b.iter(|| {
            let report = check_project(black_box(&independent_graph));
            assert_eq!(report.failure_count(), 0);
            black_box(report)
        })
    });

    let generic = Project::new("generic").independent_generic(INDEPENDENT_FILES, 10);
    let generic_graph = build(&generic);
    c.bench_function("check_project/generic_200_files", |b| {
        b.iter(|| {
            let report = check_project(black_box(&generic_graph));
            assert_eq!(report.failure_count(), 0);
            black_box(report)
        })
    });

    // Imports order the work into layers, and a layer waits for the one before it, so
    // this is what the layer barrier costs against the independent project above.
    let layered = layered();
    let layered_graph = build(&layered);
    c.bench_function("check_project/layered_201_files", |b| {
        b.iter(|| {
            let report = check_project(black_box(&layered_graph));
            assert_eq!(report.failure_count(), 0);
            black_box(report)
        })
    });

    // No parallelism to find: one file per layer.
    let chain = Project::new("chain").chain(CHAIN_LENGTH);
    let chain_graph = build(&chain);
    c.bench_function("check_project/chain_100_files", |b| {
        b.iter(|| {
            let report = check_project(black_box(&chain_graph));
            assert_eq!(report.failure_count(), 0);
            black_box(report)
        })
    });

    // Every file in one cycle, which is one layer, so the files run in parallel even
    // though they depend on each other.
    let ring = Project::new("ring-check").ring(RING_SIZE);
    let ring_graph = build(&ring);
    c.bench_function("check_project/ring_50_files", |b| {
        b.iter(|| {
            let report = check_project(black_box(&ring_graph));
            assert_eq!(report.failure_count(), 0);
            black_box(report)
        })
    });
}

// The same project checked on pools of different sizes. `check_project` uses rayon's
// current pool, so `install` is what picks the size. The result is the speedup the
// layered driver actually gets and where it stops growing.
fn bench_thread_scaling(c: &mut Criterion) {
    let independent = independent();
    let independent_graph = build(&independent);
    let layered = layered();
    let layered_graph = build(&layered);

    let mut group = c.benchmark_group("check_project_threads");
    for threads in [1usize, 2, 4, 8] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .expect("thread pool should build");

        group.bench_with_input(
            BenchmarkId::new("independent_200_files", threads),
            &threads,
            |b, _| {
                b.iter(|| {
                    pool.install(|| {
                        let report = check_project(black_box(&independent_graph));
                        assert_eq!(report.failure_count(), 0);
                        black_box(report)
                    })
                })
            },
        );
        group.bench_with_input(
            BenchmarkId::new("layered_201_files", threads),
            &threads,
            |b, _| {
                b.iter(|| {
                    pool.install(|| {
                        let report = check_project(black_box(&layered_graph));
                        assert_eq!(report.failure_count(), 0);
                        black_box(report)
                    })
                })
            },
        );
    }
    group.finish();
}

// What `tsox --project` does for one invocation: a cold resolver, the graph, then
// every file checked. The numbers above split this into its parts.
fn bench_end_to_end(c: &mut Criterion) {
    let layered = layered();
    c.bench_function("end_to_end/layered_201_files", |b| {
        b.iter(|| {
            let resolver = ModuleResolver::new();
            let graph = ModuleGraph::build(black_box(&layered.entries), &resolver)
                .expect("benchmark graph should build");
            let report = check_project(&graph);
            assert_eq!(report.failure_count(), 0);
            black_box(report)
        })
    });

    let independent = independent();
    c.bench_function("end_to_end/independent_200_files", |b| {
        b.iter(|| {
            let resolver = ModuleResolver::new();
            let graph = ModuleGraph::build(black_box(&independent.entries), &resolver)
                .expect("benchmark graph should build");
            let report = check_project(&graph);
            assert_eq!(report.failure_count(), 0);
            black_box(report)
        })
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(50)
        .warm_up_time(Duration::from_secs(3))
        .measurement_time(Duration::from_secs(10))
        .noise_threshold(0.02);
    targets = bench_build,
        bench_graph_queries,
        bench_check_project,
        bench_thread_scaling,
        bench_end_to_end
}

criterion_main!(benches);
