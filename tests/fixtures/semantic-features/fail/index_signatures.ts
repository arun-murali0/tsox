interface Dict { [k: string]: number }
declare const d: Dict;
const s: string = d["x"]; // error TS2322
const lit: Dict = { a: "no" }; // error TS2322
interface Bad { [k: string]: number; a: string } // error TS2411
interface Dup { [k: string]: number; [j: string]: string } // error TS2374
declare function values<T>(x: { [k: string]: T }): T;
const w: string = values({ a: 1 }); // error TS2322
export {};
declare const dd: { [k: string]: number };
const unchecked: number = dd["x"]; // error TS2322
