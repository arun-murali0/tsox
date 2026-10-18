use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use std::hint::black_box;
use std::time::Duration;
use tsox::TypeChecker;

#[path = "support/corpus_fixtures.rs"]
mod corpus_fixtures;

use corpus_fixtures::corpus_slice;

// One iteration checks the whole slice, so a number here is the cost of that many real
// files and throughput is files per second. A file the checker refuses is counted and not
// asserted on: the corpus holds plenty of deliberately broken code, and the point is the
// time, not the verdict.
fn bench_corpus(c: &mut Criterion) {
    let files = corpus_slice(100);
    let checker = TypeChecker::new();

    let mut group = c.benchmark_group("corpus");
    group.throughput(Throughput::Elements(files.len() as u64));
    group.bench_function("check_source/every_100th_file", |b| {
        b.iter(|| {
            let mut failed = 0usize;
            for file in &files {
                match checker.check_source(black_box(&file.source), black_box(&file.name)) {
                    Ok(result) => {
                        black_box(result);
                    }
                    Err(_) => failed += 1,
                }
            }
            black_box(failed)
        })
    });
    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(20)
        .warm_up_time(Duration::from_secs(3))
        .measurement_time(Duration::from_secs(20));
    targets = bench_corpus
}

criterion_main!(benches);
