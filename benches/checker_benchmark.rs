use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use std::time::Duration;
use tsox::{FileId, TypeChecker};

#[path = "support/complex_fixtures.rs"]
mod complex_fixtures;

use complex_fixtures::{
    class_hierarchy_source, complex_source, connected_application_source,
    destructuring_heavy_source, generic_heavy_source, generic_type_reference_source,
    nested_object_source, wide_discriminated_union_source,
};

fn tiny_source() -> String {
    "const x: number = 5;".to_string()
}

fn realistic_source() -> String {
    let mut src = String::new();

    for i in 0..50 {
        src.push_str(&format!(
            "function add{i}(a: number, b: number): number {{\n\
             \treturn a + b;\n\
             }}\n\
             add{i}(1, 2);\n"
        ));
    }

    src
}

fn large_source(function_count: usize) -> String {
    let mut src = String::with_capacity(function_count * 80);

    for i in 0..function_count {
        src.push_str(&format!(
            "function fn{i}(a: number, b: number): number {{\n\
             \treturn a + b;\n\
             }}\n"
        ));
    }

    src
}

fn bench_check_source(c: &mut Criterion) {
    let checker = TypeChecker::new();

    let tiny = tiny_source();

    c.bench_function("check_source/tiny", |b| {
        b.iter(|| {
            let result = checker.check_source(black_box(&tiny), "bench_tiny.ts");

            assert!(
                result.is_ok(),
                "benchmark source failed to check: {result:?}"
            );

            black_box(result)
        })
    });

    let realistic = realistic_source();

    c.bench_function("check_source/realistic_50_functions", |b| {
        b.iter(|| {
            let result = checker.check_source(black_box(&realistic), "bench_realistic.ts");

            assert!(
                result.is_ok(),
                "benchmark source failed to check: {result:?}"
            );

            black_box(result)
        })
    });

    let large = large_source(1_000);

    c.bench_function("check_source/large_1000_functions", |b| {
        b.iter(|| {
            let result = checker.check_source(black_box(&large), "bench_large.ts");

            assert!(
                result.is_ok(),
                "benchmark source failed to check: {result:?}"
            );

            black_box(result)
        })
    });
}

fn bench_check_source_complex(c: &mut Criterion) {
    let checker = TypeChecker::new();

    macro_rules! bench_named_source {
        ($name:expr, $source:expr) => {{
            let source = $source;

            c.bench_function($name, |b| {
                b.iter(|| {
                    let result = checker.check_source(black_box(&source), "bench_complex.ts");

                    assert!(
                        result.is_ok(),
                        "benchmark source failed to check: {result:?}"
                    );

                    black_box(result)
                })
            });
        }};
    }

    bench_named_source!(
        "check_source/wide_union_50_variants",
        wide_discriminated_union_source(50)
    );

    bench_named_source!(
        "check_source/nested_objects_depth_50",
        nested_object_source(50)
    );

    bench_named_source!(
        "check_source/class_hierarchy_depth_50",
        class_hierarchy_source(50)
    );

    bench_named_source!("check_source/generic_calls_500", generic_heavy_source(500));

    bench_named_source!(
        "check_source/generic_type_refs_500",
        generic_type_reference_source(500)
    );

    bench_named_source!(
        "check_source/destructuring_500_bindings",
        destructuring_heavy_source(500)
    );

    bench_named_source!("check_source/complex_mixed_scale_50", complex_source(50));

    bench_named_source!("check_source/complex_mixed_scale_200", complex_source(200));

    bench_named_source!(
        "check_source/connected_application_scale_10",
        connected_application_source(10)
    );

    bench_named_source!(
        "check_source/connected_application_scale_50",
        connected_application_source(50)
    );

    bench_named_source!(
        "check_source/connected_application_scale_100",
        connected_application_source(100)
    );
}

fn bench_parse_and_bind(c: &mut Criterion) {
    let large = large_source(1_000);

    c.bench_function("parse_and_bind_only/large_1000_functions", |b| {
        b.iter(|| {
            let result = tsox::parse_and_bind_only(black_box(&large), "bench_large.ts");

            assert!(
                result.is_ok(),
                "benchmark source failed to parse/bind: {result:?}"
            );

            black_box(result)
        })
    });
}

// `check_source` above builds a new type arena on every call, which is what a one-shot CLI
// run pays. A session keeps its arena between calls, which is what an editor or a watch mode
// pays after the first check, and the gap between the two is the cost of starting cold.
fn bench_check_session_warm(c: &mut Criterion) {
    let checker = TypeChecker::new();

    macro_rules! bench_warm_source {
        ($name:expr, $source:expr) => {{
            let source = $source;
            let mut session = checker.session(FileId::new(0));

            c.bench_function($name, |b| {
                b.iter(|| {
                    let result = session.check_source(black_box(&source), "bench_warm.ts");

                    assert!(
                        result.is_ok(),
                        "benchmark source failed to check: {result:?}"
                    );

                    black_box(result)
                })
            });
        }};
    }

    bench_warm_source!("check_session_warm/realistic_50_functions", realistic_source());

    bench_warm_source!("check_session_warm/large_1000_functions", large_source(1_000));

    bench_warm_source!(
        "check_session_warm/connected_application_scale_50",
        connected_application_source(50)
    );
}

// A longer warm-up and measurement window and a looser noise threshold than the defaults,
// so one busy moment on the machine is less likely to be reported as a change.
criterion_group! {
    name = benches;
    config = Criterion::default()
        .warm_up_time(Duration::from_secs(3))
        .measurement_time(Duration::from_secs(10))
        .noise_threshold(0.02);
    targets = bench_check_source,
        bench_check_source_complex,
        bench_check_session_warm,
        bench_parse_and_bind
}

criterion_main!(benches);
