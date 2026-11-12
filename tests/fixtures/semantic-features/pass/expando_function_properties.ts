function foo() {}
foo.x = 1;
foo.break = 2;
foo["quoted"] = 3;
const arrow = () => 0;
arrow.count = 1;
const fexpr = function () {};
fexpr.tag = "a";
const total: number = foo.x + arrow.count;
const s: string = fexpr.tag;
export function inner() {
  function local() {}
  local.k = 1;
  return local.k;
}
