use iai_callgrind::{library_benchmark, library_benchmark_group, main};
use std::hint::black_box;
use tsox::TypeChecker;

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

#[library_benchmark]
#[bench::tiny(tiny_source())]
#[bench::realistic_50_functions(realistic_source())]
#[bench::large_1000_functions(large_source(1_000))]
#[bench::wide_union_50_variants(wide_discriminated_union_source(50))]
#[bench::nested_objects_depth_50(nested_object_source(50))]
#[bench::class_hierarchy_depth_50(class_hierarchy_source(50))]
#[bench::generic_calls_500(generic_heavy_source(500))]
#[bench::generic_type_refs_500(generic_type_reference_source(500))]
#[bench::destructuring_500_bindings(destructuring_heavy_source(500))]
#[bench::complex_mixed_scale_50(complex_source(50))]
#[bench::complex_mixed_scale_200(complex_source(200))]
#[bench::connected_application_scale_10(connected_application_source(10))]
#[bench::connected_application_scale_50(connected_application_source(50))]
#[bench::connected_application_scale_100(connected_application_source(100))]
fn check_source(source: String) {
    let checker = TypeChecker::new();

    let result = checker.check_source(black_box(&source), "bench.ts");

    assert!(
        result.is_ok(),
        "benchmark source failed to check: {result:?}"
    );

    let _ = black_box(result);
}

library_benchmark_group!(
    name = checker_group;
    benchmarks = check_source
);

main!(library_benchmark_groups = checker_group);
