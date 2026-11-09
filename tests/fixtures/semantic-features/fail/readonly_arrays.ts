declare function takesMutable(x: number[]): void;
export function f(a: readonly number[], b: ReadonlyArray<string>, c: number[]) {
  a.push(1); // error TS2339
  b.sort(); // error TS2339
  a[0] = 1; // error TS2542
  const m: number[] = a; // error TS4104
  c = a; // error TS4104
  return [m, c];
}
