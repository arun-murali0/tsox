// After `x ??= y` and `x ||= y`, the target cannot be nullish or falsy anymore:
// it reads as the truthy part of what it had joined with the right side. `x &&= y`
// keeps the falsy part, so the target may still be undefined.
function logicalNullish(x: string | undefined): number {
  x ??= "a";
  return x.length;
}

function logicalOr(x: string | undefined): number {
  x ||= "b";
  return x.length;
}

function logicalAndKeepsPossiblyFalsy(x: string | undefined): number | undefined {
  x &&= "c";
  return x?.length;
}
