export function f(): number {
  let [x, y] = [1, 2];
  const [{ a, b: { c } }] = [{ a: 1, b: { c: 2 } }];
  const [s, n] = ["x", 3];
  return x + y + a + c + s.length + n;
}
