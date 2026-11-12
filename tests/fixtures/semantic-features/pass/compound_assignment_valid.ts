// A compound write whose result fits the narrowed target is fine, and the
// target then reads as the operation's result.
function validCompound(x: number, o: { n: number }): number {
  x += 1;
  o.n *= 2;
  return x + o.n;
}
