// A class, a function and an enum name cannot be assigned to (TS2629, TS2630, TS2628), whether
// by `=` or by a compound operator.
class C {}
function f() {}
enum E { A }
C = 1; // error TS2629
f = () => {}; // error TS2630
E = 2; // error TS2628
C += 1; // error TS2629
E.A;
