//! A recursive conditional alias that grows its own arguments (the tuple-growing helpers of
//! TypeScript issue #54491) is instantiated at each step of its expansion. Working out which
//! type parameters each step mentions walked everything reachable from it afresh, so the
//! check ran for minutes. It must finish in moments, as tsc does.
//!
//! The second test is the comparison of an interface with a subinterface that redeclares
//! eleven generic methods returning itself: each level makes fresh type parameters, so the
//! pair compared is never the same twice, and the comparison was eleven to the power of its
//! depth.

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

fn finishes(source: &'static str) -> Vec<tsox::Diagnostic> {
    let (done, finished) = mpsc::channel();
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            let result = TypeChecker::new().check_source(source, "grow.ts");
            let _ = done.send(
                result
                    .map(|checked| checked.diagnostics)
                    .unwrap_or_default(),
            );
        })
        .unwrap();
    finished
        .recv_timeout(Duration::from_secs(60))
        .expect("the check did not finish within 60 seconds")
}

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

const SUBINTERFACE: &str = r#"export declare type ThenArg<T> = T extends any ? any : T extends PromiseLike<infer U> ? U : T;
export interface InterfaceA<T> {
    filter(callback: (newValue: T, oldValue: T) => boolean): InterfaceA<T>;
    map<D>(callback: (value: T) => D): InterfaceA<D>;
    await<R extends ThenArg<T>>(): InterfaceA<R>;
    awaitLatest<R extends ThenArg<T>>(): InterfaceA<R>;
    awaitOrdered<R extends ThenArg<T>>(): InterfaceA<R>;
    awaitOrdered1<R extends ThenArg<T>>(): InterfaceA<R>;
    awaitOrdered2<R extends ThenArg<T>>(): InterfaceA<R>;
    awaitOrdered3<R extends ThenArg<T>>(): InterfaceA<R>;
    awaitOrdered4<R extends ThenArg<T>>(): InterfaceA<R>;
    awaitOrdered5<R extends ThenArg<T>>(): InterfaceA<R>;
    awaitOrdered6<R extends ThenArg<T>>(): InterfaceA<R>;
    awaitOrdered7<R extends ThenArg<T>>(): InterfaceA<R>;
    awaitOrdered8<R extends ThenArg<T>>(): InterfaceA<R>;
}
export interface InterfaceB<T> extends InterfaceA<T> {
    filter(callback: (newValue: T, oldValue: T) => boolean): InterfaceB<T>;
    map<D>(callback: (value: T) => D): InterfaceB<D>;
    await<R extends ThenArg<T>>(): InterfaceB<R>;
    awaitLatest<R extends ThenArg<T>>(): InterfaceB<R>;
    awaitOrdered<R extends ThenArg<T>>(): InterfaceB<R>;
    awaitOrdered1<R extends ThenArg<T>>(): InterfaceB<R>;
    awaitOrdered2<R extends ThenArg<T>>(): InterfaceB<R>;
    awaitOrdered3<R extends ThenArg<T>>(): InterfaceB<R>;
    awaitOrdered4<R extends ThenArg<T>>(): InterfaceB<R>;
    awaitOrdered5<R extends ThenArg<T>>(): InterfaceB<R>;
    awaitOrdered6<R extends ThenArg<T>>(): InterfaceB<R>;
    awaitOrdered7<R extends ThenArg<T>>(): InterfaceB<R>;
    awaitOrdered8<R extends ThenArg<T>>(): InterfaceB<R>;
}
declare const b: InterfaceB<string>;
export const a: InterfaceA<string> = b;
"#;

#[test]
fn comparing_an_interface_with_a_subinterface_of_generic_methods_is_checked_in_moments() {
    let errors: Vec<_> = finishes(SUBINTERFACE)
        .into_iter()
        .filter(|diagnostic| diagnostic.severity == tsox::Severity::Error)
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
}

// TypeScript issue #55630: a union of four thousand string literals intersected with another
// union, assigned to a union of the same literals. Each intersection member holds a literal
// that is itself a member of the target, which settles it without comparing it with the
// other four thousand; comparing them all is millions of comparisons.
const LITERAL_UNIONS: &str = r#"
type Digits = '0' | '1' | '2' | '3' | '4' | '5' | '6' | '7';
type T1 = `${Digits}${Digits}${Digits}${Digits}` | undefined;
type T2 = { a: string } | { b: number };

function f1(x: T1, y: T1 & T2) {
    x = y;
}

function f2(x: T1 | null, y: T1 & T2) {
    x = y;
}
"#;

#[test]
fn a_big_union_of_literals_intersected_with_another_is_checked_in_moments() {
    let _ = finishes(LITERAL_UNIONS);
}
