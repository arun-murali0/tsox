// `o.x++` gives the property a new value, so what a guard narrowed it to ends,
// the rule a plain write applies.
function updateLiteral(o: { s: 1 | 2 | "x" }): void {
  if (o.s === 1) {
    o.s++;
    const check: 1 = o.s; // error TS2322
  }
}
