declare const b: Array<string>;
b.forEach((v) => v);
function f(y: Array<string>): number[] {
  y.concat(y);
  return y.map((s) => s.length);
}
class A {
  m(z: Array<string>): void {
    z.forEach((v) => v.length);
  }
}
const nested: Array<Array<number>> = [[1], [2]];
export const total = nested.reduce((n, row) => n + row.length, 0) + f(["a"]).length;
