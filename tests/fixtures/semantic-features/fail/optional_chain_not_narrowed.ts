type Thing = { foo: string | number };

function f(o: Thing | undefined, value: number | undefined) {
  if (o?.foo === value) {
    return o.foo; // error TS18048
  }
  return 0;
}
