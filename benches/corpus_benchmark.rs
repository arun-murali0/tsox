use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use std::hint::black_box;
use std::time::Duration;
use tsox::TypeChecker;

#[path = "support/corpus_fixtures.rs"]
mod corpus_fixtures;

use corpus_fixtures::{Class, CorpusFile, corpus_class, corpus_part, corpus_slice};

// One iteration checks the whole set, so a number here is the cost of that many real files and
// throughput is files per second. A file the checker refuses is counted and not asserted on: the
// corpus holds plenty of deliberately broken code, and the point is the time, not the verdict.
fn check_all(checker: &TypeChecker, files: &[CorpusFile]) -> usize {
    let mut failed = 0usize;
    for file in files {
        match checker.check_source(black_box(&file.source), black_box(&file.name)) {
            Ok(result) => {
                black_box(result);
            }
            Err(_) => failed += 1,
        }
    }
    failed
}

fn bench_set(c: &mut Criterion, group_name: &str, id: &str, files: Vec<CorpusFile>) {
    let checker = TypeChecker::new();
    let mut group = c.benchmark_group(group_name);
    group.throughput(Throughput::Elements(files.len() as u64));
    group.bench_function(id, |b| b.iter(|| black_box(check_all(&checker, &files))));
    group.finish();
}

fn bench_corpus(c: &mut Criterion) {
    // A spread over the whole corpus, as before.
    bench_set(
        c,
        "corpus",
        "check_source/every_100th_file",
        corpus_slice(100),
    );
    // The quiet path and the diagnostic path, apart: files the map says draw nothing, and files
    // on which tsc and tsox report the same diagnostics.
    bench_set(
        c,
        "corpus_class",
        "clean_files",
        corpus_class(Class::Clean, 150),
    );
    bench_set(
        c,
        "corpus_class",
        "agreeing_error_files",
        corpus_class(Class::Errors, 150),
    );
    // One whole part: the unit the map and CI work in (TSOX_CORPUS_PART picks which).
    bench_set(c, "corpus_part", "whole_part", corpus_part());
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(10)
        .warm_up_time(Duration::from_secs(3))
        .measurement_time(Duration::from_secs(20));
    targets = bench_corpus
}

criterion_main!(benches);
