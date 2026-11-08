declare const s: string;
declare const n: number;
declare const b: boolean;
declare const u: string | number;
declare const lit: "a" | "b";
declare const one: "a";
declare const obj: { x: number };
declare const arr: number[];
export const r1 = s === n; // error TS2367
export const r2 = n === s; // error TS2367
export const r3 = s === "a";
export const r4 = one === "b"; // error TS2367
export const r5 = one === "a";
export const r6 = lit === "c"; // error TS2367
export const r7 = n !== "x"; // error TS2367
export const r8 = b == n; // error TS2367
export const r9 = obj === n; // error TS2367
export const r10 = arr === s; // error TS2367
export const r11 = u === n;
export const r12 = s === null;
export const r13 = s === undefined;
export const r14 = 1 === 2; // error TS2367
export const r15 = "a" === "b"; // error TS2367
export const r16 = n === 1;
export const r17 = b === true;
export const r18 = obj === obj;
export function f(x: string) { return x === 5; } // error TS2367
let w = "a"; export const r19 = w === "b";
const k = "a"; export const r20 = k === "b"; // error TS2367

export function sameNameElsewhere(): void {
  const x = 1;
  if (x == 1) { return; }
}
export function sameNameOther(): void {
  const x = 1;
  if (x == 2) { } // error TS2367
}
