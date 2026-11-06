// Which statements are dead, and which `case` falls through, checked against tsc.
//
// Problem: TS7027 was decided by a walker that approximated tsc's binder, so a constant
// condition, a `finally`, a loop with no way out or a run of dead statements broken by a
// function declaration each came out differently from tsc.
// Picked: `reachability` replays the binder's rules (see its module comment) and both TS7027
// and TS7029 read its answer.
// Now: each case below pins the exact lines tsc reports for it. The expected lists were taken
// from tsc 6.0.3 with `allowUnreachableCode: false` and `noFallthroughCasesInSwitch`, by
// scripts/flow-stress, not written by hand, so a change here is a change from tsc.
//
// Only TS7027 and TS7029 are read. A line that tsc reports twice (two clauses on one line)
// is listed twice.

use tsox::{CheckOptions, DiagnosticCode, FileId, UnitContext, Worker};

fn lines(source: &str, code: DiagnosticCode) -> Vec<usize> {
    let options = CheckOptions {
        allow_unreachable_code: false,
        no_fallthrough_cases_in_switch: true,
        ..CheckOptions::default()
    };
    let mut worker = Worker::new();
    let ctx = UnitContext::new(FileId::ROOT, "t.ts", source).with_options(options);
    let outcome = worker.check_unit(&ctx).expect("the unit checks");
    let mut found: Vec<usize> = outcome
        .diagnostics
        .iter()
        .filter(|d| d.code == code)
        .map(|d| source[..d.start as usize].matches('\n').count() + 1)
        .collect();
    found.sort_unstable();
    found
}

fn assert_matches_tsc(source: &str, unreachable: &[usize], fallthrough: &[usize]) {
    assert_eq!(
        lines(source, DiagnosticCode::UnreachableCode),
        unreachable,
        "TS7027 lines"
    );
    assert_eq!(
        lines(source, DiagnosticCode::FallthroughCaseInSwitch),
        fallthrough,
        "TS7029 lines"
    );
}

#[test]
fn constants_only_bare_keywords() {
    assert_matches_tsc(
        r####"declare function m(n: number): void;
declare const c: boolean;
declare const n: number;
function f1() { if (true) { m(1); } else { m(2); } m(3); }
function f2() { if ((true)) { m(1); } else { m(2); } }
function f3() { if (!true) { m(1); } else { m(2); } }
function f4() { if (1) { m(1); } else { m(2); } }
function f5() { if (c || true) { m(1); } else { m(2); } }
function f6() { if (c && false) { m(1); } else { m(2); } }
function f7() { if (true ?? c) { m(1); } else { m(2); } }
function f8() { if (!(c || true)) { m(1); } else { m(2); } }
function f9() { while (false) { m(1); } m(2); }
"####,
        &[4, 8, 9, 11, 12],
        &[],
    );
}

#[test]
fn runs_end_at_non_executable_statements() {
    assert_matches_tsc(
        r####"declare function m(n: number): void;
declare const c: boolean;
declare const n: number;
function f() {
  return;
  m(1);
  m(2);
  function g() {}
  m(3);
  ;
  m(4);
  var v;
  m(5);
  var w = 1;
  m(6);
  { m(7); }
  m(8);
  const enum E { A }
  m(9);
}
"####,
        &[6, 9, 11, 13, 16, 17, 19],
        &[],
    );
}

#[test]
fn loops() {
    assert_matches_tsc(
        r####"declare function m(n: number): void;
declare const c: boolean;
declare const n: number;
function f1() { while (true) { m(1); } m(2); }
function f2() { while (true) { if (c) break; } m(2); }
function f3() { for (;;) { m(1); } m(2); }
function f4() { do { m(1); } while (true); m(2); }
function f5() { do { continue; } while (false); m(1); }
function f6() { for (const x of [1]) { return; } m(1); }
function f7() { L: while (true) { while (true) { break L; } } m(1); }
function f8() { while (true) { try { break; } finally { m(1); } } m(2); }
function f9() { for (let i = 0; ; i++) { m(1); } m(2); }
"####,
        &[4, 6, 7, 12],
        &[],
    );
}

#[test]
fn try_catch_finally() {
    assert_matches_tsc(
        r####"declare function m(n: number): void;
declare const c: boolean;
declare const n: number;
function f1() { try { return; } finally { m(1); } m(2); }
function f2() { try { return; } catch { m(1); } m(2); }
function f3() { try { m(0); } finally { return; } m(2); }
function f4() { while (c) { try { break; } finally { m(1); } } m(2); }
function f5() { try { return; } catch { return; } m(1); }
function f6() { try { throw 1; } catch { m(1); } m(2); }
"####,
        &[4, 6, 8],
        &[],
    );
}

#[test]
fn switch_and_fallthrough() {
    assert_matches_tsc(
        r####"declare function m(n: number): void;
declare const c: boolean;
declare const n: number;
function f1() { switch (n) { case 1: return; default: return; } m(1); }
function f2() { switch (n) { case 1: return; case 2: return; } m(1); }
function f3() { switch (n) { case 1: m(1); case 2: m(2); break; default: m(3); } }
function f4() { switch (n) { case 1: case 2: m(1); break; } }
function f5() { switch (n) { case 1: m(1); default: m(2); case 3: m(3); } }
function f6() { switch (n) { default: return; } m(1); }
"####,
        &[4, 9],
        &[6, 8, 8],
    );
}

#[test]
fn labels_functions_and_classes() {
    assert_matches_tsc(
        r####"declare function m(n: number): void;
declare const c: boolean;
declare const n: number;
function f1() { L: { break L; m(1); } m(2); }
function f2() { return; (() => { m(1); return; m(2); })(); }
function f3() { return; class K { x() { return; m(1); } } }
function f4() { return; const g = function () { return; m(1); }; }
function f5() { (() => { return; })(); m(1); }
function f6() { (async () => { return; })(); m(1); }
function f7() { return; class S { static { m(1); throw 1; m(2); } } }
"####,
        &[4, 5, 5, 6, 7, 7, 10],
        &[],
    );
}

#[test]
fn top_level_namespace() {
    assert_matches_tsc(
        r####"declare function m(n: number): void;
declare const c: boolean;
declare const n: number;
export {};
throw new Error();
namespace N { export const x = 1; while (true) {} m(1); }
m(2);
"####,
        &[6],
        &[],
    );
}
