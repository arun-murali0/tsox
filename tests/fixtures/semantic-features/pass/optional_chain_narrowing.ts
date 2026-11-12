type Thing = { foo: string | number; bar(): number };

function equalsValue(o: Thing | undefined, value: number) {
  if (o?.foo === value) {
    return o.foo;
  }
  return 0;
}

function typeofChain(o: Thing | undefined) {
  if (typeof o?.foo === "number") {
    return o.foo;
  }
  return 0;
}

function truthy(o: Thing | null) {
  if (o?.foo) {
    return o.foo;
  }
  return 0;
}

function againstUndefined(o: Thing | undefined) {
  if (o?.foo !== undefined) {
    return o.foo;
  }
  return 0;
}
