use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tsox::{
    FileId, IncrementalDatabase, ModuleGraph, ModuleResolver, SourceKind, WorkerScratch, source_len,
};

#[path = "support/project_fixtures.rs"]
mod project_fixtures;

use project_fixtures::Project;

const LAYERS: usize = 20;
const WIDTH: usize = 10;
const FAN_OUT: usize = 3;
const RING_SIZE: usize = 50;
const SCRATCH_VALUES: u32 = 1_000;
const INCREMENTAL_FILES: u32 = 200;

fn build(project: &Project) -> ModuleGraph {
    ModuleGraph::build(&project.entries, &ModuleResolver::new())
        .expect("benchmark graph should build")
}

// The graph's own cycle search and petgraph's find the same cycles, and a test holds
// them to it. Both are timed on the same ring so the cost of the petgraph version is
// visible next to the one it shadows.
fn bench_topology(c: &mut Criterion) {
    let ring = Project::new("foundation-ring").ring(RING_SIZE);
    let ring_graph = build(&ring);
    let ring_topology = ring_graph.topology();

    c.bench_function("topology/cycles_graph_ring_50", |b| {
        b.iter(|| black_box(ring_graph.cycles()))
    });
    c.bench_function("topology/cycles_petgraph_ring_50", |b| {
        b.iter(|| black_box(ring_topology.cycles()))
    });

    let layered = Project::new("foundation-layered").layered(LAYERS, WIDTH, FAN_OUT);
    let layered_graph = build(&layered);
    let layered_topology = layered_graph.topology();

    c.bench_function("topology/from_graph_layered_201_files", |b| {
        b.iter(|| black_box(layered_graph.topology()))
    });
    c.bench_function("topology/components_layered_201_files", |b| {
        b.iter(|| black_box(layered_topology.components()))
    });
}

// A job's worth of small values, from the scratchpad and from a Vec that is dropped.
// Same work and same count, so the difference is the allocator and the free.
fn bench_scratchpad(c: &mut Criterion) {
    let mut scratch = WorkerScratch::with_capacity(16 * 1024);

    c.bench_function("scratchpad/alloc_1000_values_then_reset", |b| {
        b.iter(|| {
            for value in 0..SCRATCH_VALUES {
                black_box(scratch.alloc(value));
            }
            scratch.reset();
        })
    });

    c.bench_function("scratchpad/vec_1000_values_then_drop", |b| {
        b.iter(|| {
            let mut values = Vec::new();
            for value in 0..SCRATCH_VALUES {
                values.push(black_box(value));
            }
            black_box(values)
        })
    });
}

// What a save costs the query layer. An unchanged save has to be cheap, since most saves
// change nothing; a read of a result that is still valid should be close to a lookup.
fn bench_incremental(c: &mut Criterion) {
    let text: Arc<str> = Arc::from("export const value: number = 1;\n");
    let mut db = IncrementalDatabase::new();
    let files: Vec<_> = (0..INCREMENTAL_FILES)
        .map(|index| {
            db.upsert_source(
                FileId::new(index),
                PathBuf::from(format!("file_{index}.ts")),
                Arc::clone(&text),
                u64::from(index),
                SourceKind::Project,
            )
        })
        .collect();

    c.bench_function("incremental/upsert_unchanged_200_files", |b| {
        b.iter(|| {
            for index in 0..INCREMENTAL_FILES {
                black_box(db.upsert_source(
                    FileId::new(index),
                    PathBuf::from(format!("file_{index}.ts")),
                    Arc::clone(&text),
                    u64::from(index),
                    SourceKind::Project,
                ));
            }
        })
    });

    for &file in &files {
        source_len(&db, file);
    }
    c.bench_function("incremental/read_valid_result_200_files", |b| {
        b.iter(|| {
            for &file in &files {
                black_box(source_len(&db, file));
            }
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
    targets = bench_topology,
        bench_scratchpad,
        bench_incremental
}

criterion_main!(benches);
