// Number operands to shifts and bitwise operators are fine and yield a number.
function shifts(n: number): number {
  return ((n << 2) | (n >>> 1)) & 15;
}

function masks(low: number, high: number): number {
  return (low & 0xff) ^ (high | 0x100);
}
