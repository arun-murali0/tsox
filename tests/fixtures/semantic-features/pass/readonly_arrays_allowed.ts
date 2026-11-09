declare function takesReadonly(x: readonly number[]): void;
export function g(a: readonly number[], b: ReadonlyArray<string>, c: number[]) {
  const total = a.length + a.map((x) => x).length + b.indexOf("x") + a.slice(1).length;
  for (const x of a) { total + x; }
  const [h] = a;
  const r: readonly number[] = c;
  takesReadonly(c);
  const copy = [...a];
  const joined = ([] as number[]).concat(a);
  return [total, h, r, copy, joined, a.filter((x) => x > 0), a.includes(1), a[0]];
}
