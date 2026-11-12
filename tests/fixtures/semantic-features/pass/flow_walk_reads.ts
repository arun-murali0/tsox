declare function needString(x: string): void;
declare function needNumber(x: number): void;
declare function needNull(x: null): void;
declare function needUndef(x: undefined): void;
declare function needStrNum(x: string | number): void;
declare const c: boolean;

export function afterAssignment(u: string | number | null) {
  u = "a";
  needString(u);
  if (c) {
    u = 1;
  }
  needStrNum(u);
}

export function joinsBranches(u: string | number) {
  if (typeof u === "string") {
    needString(u);
  } else {
    needNumber(u);
  }
  needStrNum(u);
}

export function whileLoopSettles() {
  let x: string | number | boolean = 0;
  while (c) {
    if (typeof x === "number") {
      x = "abc";
    } else {
      x = x.slice(1);
    }
  }
}

export function constantTrueLoop(u: string | null) {
  while (true) {
    if (u === null) {
      u = "x";
    }
    needString(u);
    if (c) break;
  }
}

export function switchNarrows(u: string | number | null) {
  switch (typeof u) {
    case "string":
      needString(u);
      break;
    case "number":
      needNumber(u);
      break;
    default:
      needNull(u);
  }
}

export function switchOnLiteral(k: "a" | "b" | undefined) {
  switch (k) {
    case "a":
      return 1;
    case undefined:
      needUndef(k);
      return 2;
    default:
      return 3;
  }
}

export function forLoopBreak(u: string | number) {
  for (let i = 0; i < 3; i++) {
    if (typeof u !== "string") continue;
    needString(u);
  }
}

export function unknownNarrowed(v: unknown) {
  if (typeof v === "string") {
    needString(v);
  }
  if (v === null) {
    needNull(v);
  }
}

export function earlyReturn(u: string | undefined) {
  if (u === undefined) {
    return;
  }
  needString(u);
}

export function doWhile(u: string | number) {
  do {
    u = "s";
  } while (c);
  needString(u);
}

export function labeled(u: string | number) {
  outer: for (;;) {
    if (typeof u === "string") break outer;
    u = "done";
  }
  needString(u);
}
