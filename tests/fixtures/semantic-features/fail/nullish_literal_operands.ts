// A literal `null` or `undefined` as an operand is TS18050 on the operand itself, and that side
// then adds no operand error of its own. A string on the other side of `+` concatenates and
// accepts anything.
const bitwise = 4 | null; // error TS18050
const shifted = undefined << 1; // error TS18050
const product = 2 * (null); // error TS2531
const sum = null + 1; // error TS18050
const both = null - undefined; // error TS18050
const both2 = 0 > 0 ? 1 : 2;
const compared = 1 < undefined; // error TS18050
const concatenated = "s" + null;
const concatenated2 = undefined + "s";
const typed = 1 - (null as number | null); // error TS2531


// The unary operators and `in` take the same operands.
const negated = -null; // error TS18050
const inverted = ~undefined; // error TS18050
const plus = +null; // error TS18050
const inRight = "a" in null; // error TS18050
const inLeft = null in {}; // error TS18050
