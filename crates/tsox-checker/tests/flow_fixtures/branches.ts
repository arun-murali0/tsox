// if / else, short-circuit operands, ternary arms, nullish coalescing.
export function branches(x: string | null, y: number) {
  if (x !== null && y > 0) {
    x;
  } else {
    y;
  }
  const z = x ?? "d";
  z;
  return x ? x.length : y;
}
