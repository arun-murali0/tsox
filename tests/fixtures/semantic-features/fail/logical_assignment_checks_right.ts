// The right side of a logical assignment is checked against the target's type,
// since the write does happen whenever the operator's condition says so.
function logicalNullishMismatch(x: string | undefined): void {
  x ??= 1; // error TS2322
}

function logicalOrMismatch(x: string): void {
  x ||= 2; // error TS2322
}
