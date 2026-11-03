// nested functions are bodies of their own; code after return is dead.
export function outer(x: string | null) {
  const f = (y: number) => { if (y) return 1; };
  function g() { return x; }
  return f(1);
  x;
}
export function falls(x: number) {
  if (x) { return 1; }
}
