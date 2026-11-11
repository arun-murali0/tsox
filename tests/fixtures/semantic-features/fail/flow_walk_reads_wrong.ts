declare function needString(x: string): void;
declare function needNumber(x: number): void;
declare const c: boolean;

export function branchLeavesUnion(u: string | number) {
  if (c) {
    u = "a";
  }
  needString(u); // error TS2345
}

export function wrongBranch(u: string | number) {
  if (typeof u === "string") {
    needNumber(u); // error TS2345
  } else {
    needString(u); // error TS2345
  }
}

export function loopWidens() {
  let x: string | number = "a";
  while (c) {
    needString(x); // error TS2345
    x = 1;
  }
}

export function afterLoop(u: string | number | null) {
  while (u === null) {
    u = 1;
  }
  needString(u); // error TS2345
}
