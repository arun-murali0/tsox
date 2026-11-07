// Reads of a variable before it is assigned (TS2454), checked against tsc.
//
// Problem: a `let x: number;` that is read on a path where nothing assigned it was not reported
// at all, and the rules for when tsc does report it are not the obvious ones: `x += 1` does not
// assign, `finally` does not see what its `try` assigned, a closure is excused only if something
// assigns the variable with a plain `=`, and a declared type that already holds `undefined` is
// never reported.
// Picked: the structural flow in `reachability` carries what is definitely assigned, records the
// reads that can be reached without an assignment, and the checker reports those whose declared
// type allows it.
// Now: each case below pins the exact lines tsc 6.0.3 reports for it under `strict`, taken from
// tsc by scripts/flow-stress rather than written by hand. A line tsc reports twice is listed
// twice.

use tsox::{CheckOptions, DiagnosticCode, FileId, UnitContext, Worker};

fn lines(source: &str) -> Vec<usize> {
    let options = CheckOptions {
        strict: true,
        strict_null_checks: true,
        ..CheckOptions::default()
    };
    let mut worker = Worker::new();
    let ctx = UnitContext::new(FileId::ROOT, "t.ts", source).with_options(options);
    let outcome = worker.check_unit(&ctx).expect("the unit checks");
    let mut found: Vec<usize> = outcome
        .diagnostics
        .iter()
        .filter(|d| d.code == DiagnosticCode::UsedBeforeAssigned)
        .map(|d| source[..d.start as usize].matches('\n').count() + 1)
        .collect();
    found.sort_unstable();
    found
}

#[test]
fn paths_must_all_assign() {
    assert_eq!(
        lines(
            r####"declare function m(n: number): void;
declare function ms(s: string): void;
declare function mz(n: number | undefined): void;
declare const c: boolean;
declare const sv: number;
function f1() { let x: number; m(x); }
function f2() { let x: number; if (c) x = 1; m(x); }
function f3() { let x: number; if (c) x = 1; else x = 2; m(x); }
function f4() { let x: number; x = 1; m(x); }
function f5() { let x: number; while (c) { x = 1; } m(x); }
function f6() { let x: number; do { x = 1; } while (c); m(x); }
function f7() { let x: number; for (;;) { x = 1; break; } m(x); }
function f8() { let x: number; for (const k of [1]) { x = 1; } m(x); }
function f9() { let x: number; switch (sv) { case 0: x = 1; break; default: x = 2; } m(x); }
function f10() { let x: number; switch (sv) { case 0: x = 1; break; } m(x); }
function f11() { let x: number; for (x of [1]) { m(x); } m(x); }
function f12() { let x: number; [x] = [1]; m(x); }
function f13() { let x: number; while (c) { let t: number; m(t); t = 1; m(t); } }
function f14() { m(x); var x: number; }
"####
        ),
        [6, 7, 10, 13, 15, 16, 18, 19]
    );
}

#[test]
fn constants_and_short_circuit() {
    assert_eq!(
        lines(
            r####"declare function m(n: number): void;
declare function ms(s: string): void;
declare function mz(n: number | undefined): void;
declare const c: boolean;
declare const sv: number;
function f1() { let x: number; if (true) x = 1; m(x); }
function f2() { let x: number; if (false) x = 1; m(x); }
function f3() { let x: number; c && (x = 1); m(x); }
function f4() { let x: number; c || (x = 1); m(x); }
function f5() { let x: number; (x = 1) && c; m(x); }
function f6() { let x: number; c ? (x = 1) : (x = 2); m(x); }
function f7() { let x: number; true && (x = 1); m(x); }
function f8() { let x: number; x = c ? 1 : 2; m(x); }
function f9() { let x: number; if (c && (x = 1)) { m(x); } m(x); }
"####
        ),
        [7, 8, 9, 14]
    );
}

#[test]
fn try_catch_finally() {
    assert_eq!(
        lines(
            r####"declare function m(n: number): void;
declare function ms(s: string): void;
declare function mz(n: number | undefined): void;
declare const c: boolean;
declare const sv: number;
function f1() { let x: number; try { x = 1; } catch { } m(x); }
function f2() { let x: number; try { x = 1; } catch { return; } m(x); }
function f3() { let x: number; try { x = 1; } finally { } m(x); }
function f4() { let x: number; try { x = 1; } finally { m(x); } }
function f5() { let x: number; try { } finally { x = 1; } m(x); }
function f6() { let x: number; try { x = 1; } catch { x = 2; } m(x); }
"####
        ),
        [6, 9]
    );
}

#[test]
fn compound_assignment_does_not_assign() {
    assert_eq!(
        lines(
            r####"declare function m(n: number): void;
declare function ms(s: string): void;
declare function mz(n: number | undefined): void;
declare const c: boolean;
declare const sv: number;
function f1() { let x: number; x++; m(x); }
function f2() { let x: number; x += 1; m(x); }
function f3() { let x: number; x ||= 1; m(x); }
function f4() { let x: number; x &&= 1; m(x); }
function f5() { let x: number; x ??= 1; m(x); }
"####
        ),
        [6, 6, 7, 7, 9]
    );
}

#[test]
fn closures_follow_let_and_assignment() {
    assert_eq!(
        lines(
            r####"declare function m(n: number): void;
declare function ms(s: string): void;
declare function mz(n: number | undefined): void;
declare const c: boolean;
declare const sv: number;
function f1() { let x: number; function g() { m(x); } }
function f2() { let x: number; const g = () => { m(x); }; }
function f3() { var x: number; const g = () => { m(x); }; }
function f4() { let x: number; const g = () => { m(x); }; x = 1; }
function f5() { let x: number; (() => { m(x); })(); }
function f6() { let x: number; (async () => { m(x); })(); }
function f7() { let x: number; x = 1; (async () => { m(x); })(); }
function f8() { let x: number; x++; const g = () => { m(x); }; }
"####
        ),
        [6, 7, 10, 11, 13, 13]
    );
}

#[test]
fn narrowing_by_the_test() {
    assert_eq!(
        lines(
            r####"declare function m(n: number): void;
declare function ms(s: string): void;
declare function mz(n: number | undefined): void;
declare const c: boolean;
declare const sv: number;
function f1() { let x: number; if (x !== undefined) { m(x); } }
function f2() { let x: number; if (x) { m(x); } m(x); }
function f3() { let x: number; if (typeof x === "number") { m(x); } }
function f4() { let x: number; if (!x) { return; } m(x); }
function f5() { let x: number; if (x === undefined) { m(x); } else { m(x); } }
"####
        ),
        [6, 7, 7, 8, 9, 10, 10]
    );
}

#[test]
fn declared_type_decides() {
    assert_eq!(
        lines(
            r####"declare function m(n: number): void;
declare function ms(s: string): void;
declare function mz(n: number | undefined): void;
declare const c: boolean;
declare const sv: number;
function f1() { let x: number | undefined; mz(x); }
function f2() { let x: any; m(x); }
function f3() { let x: unknown; m(x as number); }
function f4() { let x!: number; m(x); }
function f5() { let x: string; ms(x); }
function f6() { let x: void; mz(x as undefined); }
"####
        ),
        [10]
    );
}
