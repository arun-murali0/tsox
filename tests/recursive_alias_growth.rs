//! A recursive conditional alias that grows its own arguments (the tuple-growing helpers of
//! TypeScript issue #54491) is instantiated at each step of its expansion. Working out which
//! type parameters each step mentions walked everything reachable from it afresh, so the
//! check ran for minutes. It must finish in moments, as tsc does.

use std::sync::mpsc;
use std::time::Duration;

use tsox::TypeChecker;

const SOURCE: &str = r#"
type UnshiftTuple<T extends [...any[]]> = T extends [T[0], ...infer Tail] ? Tail : never;
type ExpandSmallerTuples<T extends [...any[]]> = T extends [T[0], ...infer Tail] ? T | ExpandSmallerTuples<Tail> : [];
type Shift<A extends Array<any>> = ((...args: A) => void) extends (...args: [A[0], ...infer R]) => void ? R : never;
type GrowExpRev<A extends Array<any>, N extends number, P extends Array<Array<any>>> = A['length'] extends N ? A : GrowExpRev<[...A, ...P[0]][N] extends undefined ? [...A, ...P[0]] : A, N, Shift<P>>;
type GrowExp<A extends Array<any>, N extends number, P extends Array<Array<any>>> = [...A, ...A][N] extends undefined ? GrowExp<[...A, ...A], N, [A, ...P]> : GrowExpRev<A, N, P>;
type Tuple<T, N extends number> = number extends N ? Array<T> : N extends 0 ? [] : N extends 1 ? [T] : GrowExp<[T], N, [[]]>;

declare class ArrayValidator<T extends unknown[], I = T[number]> {
    lengthRange<S extends number, E extends number>(start: S, endBefore: E): ArrayValidator<Exclude<ExpandSmallerTuples<UnshiftTuple<[...Tuple<I, E>]>>, ExpandSmallerTuples<UnshiftTuple<[...Tuple<I, S>]>>>>;
}
"#;

#[test]
fn a_growing_recursive_alias_is_checked_in_moments() {
    let (done, finished) = mpsc::channel();
    // A thread of its own with the stack the checker is given elsewhere. If the check does not
    // finish, the test fails on the timeout and the thread is left behind.
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            let result = TypeChecker::new().check_source(SOURCE, "grow.ts");
            let _ = done.send(result.is_ok());
        })
        .unwrap();
    let ok = finished
        .recv_timeout(Duration::from_secs(60))
        .expect("the check did not finish within 60 seconds");
    assert!(ok);
}
