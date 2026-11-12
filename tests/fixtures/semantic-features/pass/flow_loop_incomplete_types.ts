declare const c: boolean;
export function d() {
  let x: string | number | boolean = 0;
  while (c) {
    if (typeof x === "number") {
      x = "abc";
    } else {
      x = x.slice(1);
    }
  }
}
export function e() {
  let x: string | number | boolean = 0;
  while (c) {
    if (typeof x === "number") {
      x = "abc";
    } else {
      x;
      x = "q";
    }
  }
}
