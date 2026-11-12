function f(x: string | undefined) {
  switch (true) {
    case typeof x !== "undefined":
      return x.length;
    default:
      return 0;
  }
}

function g(x: string | number) {
  switch (true) {
    case typeof x === "string":
      return x.length;
    case typeof x === "number":
      return x.toFixed();
  }
  return 0;
}
