// Shifts and bitwise operators check their operands like the arithmetic ones do,
// under the same codes tsc reports (TS2362 and TS2363), and yield a number.
function shifts(s: string, n: number): number {
  return (s << 1) | n; // error TS2362
}

function bitwise(flag: boolean): number {
  return flag & 1; // error TS2362
}

function rightSide(n: number): number {
  return n >> "4"; // error TS2363
}
