// assignments order against reads; branch joins; member paths.
export function assign(a: string | number, o: { p: { q: string | null } }) {
  let v: string | number = a;
  v;
  v = v + "x";
  v;
  if (a) {
    v = 1;
  }
  v;
  o.p.q = "s";
  o.p.q;
}
