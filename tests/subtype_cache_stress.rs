// Stress and repeatability coverage for the (source, target) subtype cache
// added to CheckContext::relation_cache / SemanticQueries. These tests don't
// touch the cache's internals directly (it is private, reached only through
// ctx.semantic()); they go through the public TypeChecker API and try to
// break the cache the way an internal implementation bug would actually
// surface: a wrong diagnostic, a diagnostic that only appears sometimes, or a
// count that drifts across repeated runs of the exact same source.

use std::collections::HashSet;

use tsox::{Diagnostic, Severity, TypeChecker};

fn check(source: &str, file_name: &str) -> Vec<Diagnostic> {
    let checker = TypeChecker::new();
    let result = checker.check_source(source, file_name);
    assert!(result.is_ok(), "source should at least parse: {result:?}");
    result
        .map(|checked| checked.diagnostics)
        .unwrap_or_default()
}

// A stable fingerprint of a diagnostic set: (code, start) pairs, sorted. Two
// runs over the same source must produce the exact same set every time; a
// caching bug that returns a stale or wrong answer would most likely show up
// as either a missing/extra diagnostic or a shifted span.
fn fingerprint(diagnostics: &[Diagnostic]) -> Vec<(String, u32)> {
    let mut fp: Vec<(String, u32)> = diagnostics
        .iter()
        .map(|d| (format!("{:?}", d.code), d.start))
        .collect();
    fp.sort();
    fp
}

fn assert_identical_across_runs(source: &str, file_name: &str, runs: usize) -> Vec<Diagnostic> {
    let first = check(source, file_name);
    let first_fp = fingerprint(&first);

    for run in 1..runs {
        let repeat = check(source, file_name);
        let repeat_fp = fingerprint(&repeat);
        assert_eq!(
            first_fp, repeat_fp,
            "run {run} of {runs} produced different diagnostics than run 0 for {file_name}\n\
             (each run uses a fresh TypeChecker/CheckContext, so the cache is never shared \
             across runs -- a mismatch here means the cache changed behavior within a single \
             run in a way that depends on something it shouldn't, e.g. hash iteration order)"
        );
    }

    first
}

// Many arguments bound to the same type parameter within one call. Each
// argument after the first re-checks its candidate against the *same*
// existing TypeId (see semantic::generics::infer_type_param_bindings);
// this is the clearest same-TypeId repeat the cache is meant to catch, so if
// the cache ever returns a stale answer, this is where it would show up
// first: as a widened-too-far or not-widened-enough inferred T.
#[test]
fn generic_function_with_many_same_typed_arguments_stays_correct() {
    let source = r#"
        function allSame<T>(a: T, b: T, c: T, d: T, e: T, f: T, g: T, h: T): T {
            return a;
        }

        // Every argument is a number: T should infer to number, no diagnostics.
        allSame(1, 2, 3, 4, 5, 6, 7, 8);
    "#;
    let diagnostics =
        assert_identical_across_runs(source, "generic_many_same_typed_arguments_ok.ts", 20);
    assert!(
        diagnostics.is_empty(),
        "expected all-number arguments to infer T = number cleanly, got: {diagnostics:?}"
    );
}

// Same shape, but one argument in the middle is a mismatched type. If the
// cache ever conflated this call's (candidate, existing) pair with a
// leftover entry from a structurally similar but distinct check, the
// mismatch could either go unreported (false negative) or get reported
// against the wrong argument.
#[test]
fn generic_function_with_one_mismatched_argument_among_many_is_still_caught() {
    let source = r#"
        function allSame<T>(a: T, b: T, c: T, d: T, e: T): T {
            return a;
        }

        allSame(1, 2, "not a number", 4, 5);
    "#;
    let diagnostics =
        assert_identical_across_runs(source, "generic_many_arguments_one_mismatch.ts", 20);
    assert!(
        !diagnostics.is_empty(),
        "expected the string argument among numbers to be flagged, got no diagnostics"
    );
}

// Two *separate* generic calls in the same file, same type parameter name,
// argument types that widen to the same primitive but are different
// TypeIds (fresh literal allocations each time). This is exactly the case
// that should NOT collide in the cache: each call's inference must be
// independent, matching the existing
// two_generic_functions_share_type_param_name_without_cross_contamination
// test in tests/generics_tier1.rs, but repeated many times and interleaved
// with a mismatch to make sure no stale entry leaks between calls.
#[test]
fn repeated_independent_generic_calls_do_not_cross_contaminate() {
    let source = r#"
        function identity<T>(x: T): T {
            return x;
        }

        const a = identity(1);
        const b = identity("x");
        const c = identity(true);
        const d = identity(2);
        const e = identity("y");
        const f = identity(false);

        const wrongNumber: number = identity("still a string");
    "#;
    let diagnostics =
        assert_identical_across_runs(source, "repeated_independent_generic_calls.ts", 20);
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly the one genuine mismatch (wrongNumber), got: {diagnostics:?}"
    );
}

// Wide-union-vs-wide-union subtyping: is_subtype's union arms are
// sub_members.all(..) combined with sup_members.any(..), so this is O(N*M)
// recursive calls with no memoization inside subtyping.rs itself. Building
// several independently-typed unions and assigning between them exercises
// many distinct (TypeId, TypeId) pairs in one check, which is the shape most
// likely to reveal a cache correctness bug (as opposed to a cache
// performance question, which this test doesn't measure).
#[test]
fn wide_union_assignments_in_both_directions_stay_correct() {
    let mut source = String::from("type Wide =\n");
    for i in 0..40 {
        source.push_str(&format!("    | \"variant{i}\"\n"));
    }
    source.push_str(";\n\n");

    // A subset union: every member of Narrow is also a member of Wide, so
    // Narrow should be assignable to Wide (every sub-member matches some
    // sup-member), but not the reverse.
    source.push_str("type Narrow =\n");
    for i in 0..10 {
        source.push_str(&format!("    | \"variant{i}\"\n"));
    }
    source.push_str(";\n\n");

    // Parameters, not initialized constants: tsc narrows `const w2: Wide = "variant0"`
    // to the literal it was given, so a constant would make Wide -> Narrow legal and
    // this test would stop exercising the relation it is here for.
    source.push_str("function widen(n: Narrow, w2: Wide): void {\n");
    source.push_str("    const w1: Wide = n;\n"); // Narrow -> Wide: fine.
    source.push_str("    const n2: Narrow = w2;\n"); // Wide -> Narrow: not fine.
    source.push_str("}\n");

    let diagnostics = assert_identical_across_runs(&source, "wide_union_both_directions.ts", 10);
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic (the Wide -> Narrow assignment), got: {diagnostics:?}"
    );
}

// The same wide-union source checked many times back to back through
// independent TypeChecker instances, specifically looking for any
// nondeterminism in diagnostic ordering or count: a HashMap-backed cache
// (FxHashMap) has no guaranteed iteration order, so if any code path ever
// iterated the cache itself (rather than only doing point lookups/inserts,
// which is all is_subtype does), this is the kind of test that would catch
// the resulting flakiness.
#[test]
fn many_repeated_runs_of_a_union_heavy_source_never_flake() {
    let mut source = String::from("type Flags =\n");
    for i in 0..25 {
        source.push_str(&format!("    | \"flag{i}\"\n"));
    }
    source.push_str(";\n\nfunction useFlag(f: Flags): Flags { return f; }\n\n");
    for i in 0..25 {
        source.push_str(&format!("useFlag(\"flag{i}\");\n"));
    }
    source.push_str("useFlag(\"not-a-flag\");\n");

    let mut seen_counts: HashSet<usize> = HashSet::new();
    for _ in 0..30 {
        let diagnostics = check(&source, "union_heavy_repeated.ts");
        seen_counts.insert(diagnostics.len());
    }
    assert_eq!(
        seen_counts,
        HashSet::from([1]),
        "expected exactly one diagnostic (the \"not-a-flag\" call) on every one of 30 runs, \
         but saw diagnostic counts: {seen_counts:?}"
    );
}

// Interleaves a generic-binding-heavy section, a union-heavy section, and an
// object/class-inheritance-heavy section in one file, so the cache
// accumulates entries from all three shapes of check at once before any of
// them finishes: the scenario most likely to expose a cache key collision
// if (TypeId, TypeId) pairs from unrelated parts of the check were ever
// mixed up.
#[test]
fn mixed_generic_union_and_class_checks_in_one_file_stay_correct() {
    let source = r#"
        function pick<T>(a: T, b: T, c: T): T {
            return a;
        }

        type Status = "ok" | "warn" | "error" | "fatal" | "unknown";

        class Base {
            id: number = 0;
        }
        class Derived extends Base {
            label: string = "";
        }

        const p1 = pick(1, 2, 3);
        const p2 = pick("a", "b", "c");
        const s: Status = "warn";
        const d: Derived = new Derived();
        const asBase: Base = d;

        // Genuine errors, interleaved with the correct code above so the
        // cache has already accumulated plenty of unrelated entries by the
        // time these are checked.
        const badPick = pick(1, "two", 3);
        const badStatus: Status = "not-a-status";
        const badBase: Derived = new Base();
    "#;
    let diagnostics = assert_identical_across_runs(source, "mixed_generic_union_class.ts", 15);
    assert_eq!(
        diagnostics.len(),
        3,
        "expected exactly the three seeded errors (badPick, badStatus, badBase), got: {diagnostics:?}"
    );
    assert!(
        diagnostics.iter().all(|d| d.severity == Severity::Error),
        "expected all three to be errors, got: {diagnostics:?}"
    );
}
