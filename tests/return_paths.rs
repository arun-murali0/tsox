// Functions that can run off their end, and `return` statements with no value, checked against
// tsc: TS2355, TS2366, TS2534 and TS7030.
//
// Problem: nothing reported a function whose declared type needs a value and whose end can be
// reached, and tsc's rules for which message applies are an ordered list that depends on the
// declared type, on whether any `return` can be reached, on strict null checks and on
// `noImplicitReturns`.
// Picked: the structural flow records, per function, whether its end and a `return` can be
// reached; the checker records each function's declared or inferred return type; the report pairs
// them in tsc's order.
// Now: each case below pins the exact (line, code) pairs tsc 6.0.3 reports, taken from tsc by
// scripts/flow-stress and not written by hand. `strict` is `strict` plus `noImplicitReturns`;
// `loose` is `noImplicitReturns` without strict null checks.

use tsox::{CheckOptions, FileId, UnitContext, Worker};

fn reported(source: &str, strict: bool) -> Vec<(usize, u32)> {
    let options = CheckOptions {
        strict,
        strict_null_checks: strict,
        no_implicit_returns: true,
        ..CheckOptions::default()
    };
    let mut worker = Worker::new();
    let ctx = UnitContext::new(FileId::ROOT, "t.ts", source).with_options(options);
    let outcome = worker.check_unit(&ctx).expect("the unit checks");
    let mut found: Vec<(usize, u32)> = outcome
        .diagnostics
        .iter()
        .filter_map(|d| {
            let code = d.code.tsc_code()?;
            [2355, 2366, 2534, 7030]
                .contains(&code)
                .then(|| (source[..d.start as usize].matches('\n').count() + 1, code))
        })
        .collect();
    found.sort_unstable();
    found
}

#[test]
fn declared_types() {
    assert_eq!(
        reported(
            r####"declare const c: boolean;
function a(): number { }
function b(): number { if (c) return 1; }
function d(): number | undefined { if (c) return 1; }
function e(): void { }
function f(): never { }
function f2(): never { throw new Error(); }
function o(): any { }
function p(): number | void { if (c) return 1; }
function q(): unknown { if (c) return 1; }
function r(): undefined { }
const j = (): number => { };
const k = function (): string { if (c) return "a"; };
class K { m(): number { } }
"####,
            true
        ),
        vec![
            (2, 2355),
            (3, 2366),
            (4, 7030),
            (6, 2534),
            (10, 7030),
            (12, 2355),
            (13, 2366),
            (14, 2355)
        ]
    );
}

#[test]
fn unannotated_inference() {
    assert_eq!(
        reported(
            r####"declare const c: boolean;
function f1() { if (c) return 1; }
function f2() { return; }
function f3() { if (c) return 1; else return 2; }
function f4() { if (c) return undefined; }
function f5() { if (c) return 1; return; }
function f6() { while (true) { if (c) return 1; } }
function f7() { try { return 1; } finally { } }
function f8() { if (c) return 1; throw new Error(); }
const f9 = () => { if (c) return 1; };
class K { m() { if (c) return 1; } }
function f10() { if (c) return 1 as any; }
(() => { if (c) return 1; })();
function f11() { return 1; return 2; }
function f12() { if (c) return; }
"####,
            true
        ),
        vec![(2, 7030), (10, 7030), (11, 7030), (13, 7030)]
    );
}

#[test]
fn async_functions() {
    assert_eq!(
        reported(
            r####"declare const c: boolean;
async function a(): Promise<number> { }
async function b(): Promise<void> { }
async function d(): Promise<never> { }
async function e(): Promise<number | undefined> { if (c) return 1; }
async function f() { if (c) return 1; }
const g = async (): Promise<string> => { if (c) return "a"; };
async function h(): Promise<any> { }
async function i(): Promise<unknown> { if (c) return 1; }
"####,
            true
        ),
        vec![
            (2, 2355),
            (4, 2534),
            (5, 7030),
            (6, 7030),
            (7, 2366),
            (9, 7030)
        ]
    );
}

#[test]
fn without_strict_null_checks() {
    assert_eq!(
        reported(
            r####"declare const c: boolean;
function f1(): number { if (c) return 1; }
function f2(): number { if (c) return 1; return; }
function f3(): number { }
function f4(): void { return; }
function f5() { if (c) return 1; return; }
function f6(): number | undefined { if (c) return 1; return; }
class K { constructor() { return; } }
"####,
            false
        ),
        vec![(2, 7030), (3, 7030), (4, 2355), (6, 7030), (7, 7030)]
    );
}

#[test]
fn flow_decides_the_end() {
    assert_eq!(
        reported(
            r####"declare const c: boolean;
function f1(): number { while (true) { } }
function f2(): number { for (;;) { if (c) return 1; } }
function f3(): number { if (c) { return 1; } else { return 2; } }
function f4(): number { switch (c) { case true: return 1; default: return 2; } }
function f5(): number { switch (c) { case true: return 1; } }
function f6(): number { try { return 1; } finally { } }
function f7(): number { try { throw 1; } catch { } }
function f8(): number { return 1; return 2; }
function f9(): number { if (true) return 1; }
function f10(): number { if (false) return 1; }
"####,
            true
        ),
        vec![(6, 2366), (8, 2355), (11, 2355)]
    );
}
