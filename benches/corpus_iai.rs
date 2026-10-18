use iai_callgrind::{library_benchmark, library_benchmark_group, main};
use std::hint::black_box;
use tsox::TypeChecker;

#[path = "support/corpus_fixtures.rs"]
mod corpus_fixtures;

use corpus_fixtures::{CorpusFile, corpus_slice};

// Reading the files happens while the argument is built, outside the measured function, so
// the instruction count is the checker's own. One file in 500 is about twenty files, which
// keeps the callgrind run short; TSOX_CORPUS_STRIDE changes the slice.
#[library_benchmark]
#[bench::every_500th_file(corpus_slice(500))]
fn check_corpus_slice(files: Vec<CorpusFile>) {
    let checker = TypeChecker::new();
    let mut failed = 0usize;

    for file in &files {
        match checker.check_source(black_box(&file.source), black_box(&file.name)) {
            Ok(result) => {
                black_box(result);
            }
            Err(_) => failed += 1,
        }
    }

    black_box(failed);
}

library_benchmark_group!(
    name = corpus_group;
    benchmarks = check_corpus_slice
);

main!(library_benchmark_groups = corpus_group);
