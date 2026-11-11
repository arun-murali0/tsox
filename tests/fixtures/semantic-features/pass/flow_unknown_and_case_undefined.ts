declare function needNull(x: null): void;
declare function needUndef(x: undefined): void;
declare function needA(x: "a"): void;
declare function needNum(x: number): void;

export function unknownEquality(v: unknown) {
  if (v === null) needNull(v);
  if (v === undefined) needUndef(v);
  if (v === "a") needA(v);
  if (v === 1) needNum(v);
}

export function caseUndefined(k: "a" | "b" | undefined) {
  switch (k) {
    case "a":
      break;
    case undefined:
      needUndef(k);
      break;
  }
}

export function forEverBreak(u: string | number) {
  for (;;) {
    if (typeof u === "string") break;
    u = "done";
  }
  const s: string = u;
  return s;
}
