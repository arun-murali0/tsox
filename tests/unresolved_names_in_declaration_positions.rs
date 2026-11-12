use tsox::{Severity, TypeChecker};

// A name nothing declares is TS2304 wherever it is written: in the places the expression pass
// does not walk (heritage clauses, `typeof`, default parameters, property initializers, enum
// members, computed keys) as much as in an ordinary expression, and a qualified type name that
// starts with nothing is a missing namespace (TS2503). These shapes also draw not-yet-checked
// warnings, which the fixture harness would count, so the errors are checked here. The source
// matches tsc 6.0.3 line for line.

const SOURCE: &str = r#"// A name nothing declares is TS2304 wherever it is written: in the places the expression pass
// does not walk (heritage clauses, `typeof`, default parameters, property initializers,
// enum members, computed keys) as much as in an ordinary expression. A qualified type name
// that starts with nothing is a missing namespace (TS2503).
export {};
interface I1 extends Missing1 {} // error TS2304
class C1 extends Missing2 {} // error TS2304
class C2 implements Missing3 {} // error TS2304
declare var v1: typeof Missing4; // error TS2304
function f1(a = Missing5) {} // error TS2304
class C3 {
  p = Missing6; // error TS2304
  static s = Missing7; // error TS2304
  [Missing8] = 1; // error TS2304
}
enum E1 { A = Missing9 } // error TS2304
var d1 = { [Missing10]: 1 }; // error TS2304
declare var v2: Missing11.inner; // error TS2503
import i2 = Missing12.x; // error TS2503

// Declared names are fine in the same places.
interface Declared {}
declare const value: number;
namespace Ns { export interface T {} }
interface I2 extends Declared {}
function f2(a = value, b: Ns.T = {}) {}
enum E2 { A = 1, B = A }
declare var v3: typeof value;
"#;

#[test]
fn names_nothing_declares_are_reported_in_every_position() {
    let result = TypeChecker::new()
        .check_source(SOURCE, "case.ts")
        .expect("should parse");
    let mut found: Vec<(usize, u32)> = result
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| {
            let line = SOURCE[..d.start as usize].matches('\n').count() + 1;
            (line, d.code.tsc_code().unwrap_or(0))
        })
        .collect();
    found.sort_unstable();
    found.dedup();
    let expected: Vec<(usize, u32)> = vec![
        (6, 2304),
        (7, 2304),
        (8, 2304),
        (9, 2304),
        (10, 2304),
        (12, 2304),
        (13, 2304),
        (14, 2304),
        (16, 2304),
        (17, 2304),
        (18, 2503),
        (19, 2503),
    ];
    assert_eq!(found, expected);
}
