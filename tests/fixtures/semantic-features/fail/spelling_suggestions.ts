declare const s: string;
const a = s.toFixed(2); // error TS2551
const b = s.nope(); // error TS2339
declare const arr: number[];
const c = arr.lenght; // error TS2551
const d = arr.psh(1); // error TS2551
interface P { name: string; age: number }
declare const p: P;
const e = p.nam; // error TS2551
const g = p.zzz; // error TS2339
export {};
