// `x op= y` writes only when the operation's result fits what the target reads
// as right now — the narrowed type, not the declared one — and tsc reports the
// write when it does not (TS2322). After a rejected write the old narrowing
// survives, so the target still reads as whatever the guard had made of it.
function compoundMismatch(x: string | number): void {
  if (typeof x === "number") {
    x += "!"; // error TS2322
    const still: number = x;
  }
}

function memberCompoundMismatch(o: { s: string | number }): void {
  if (typeof o.s === "number") {
    o.s += "!"; // error TS2322
    const still: number = o.s;
  }
}
