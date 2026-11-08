declare const s: string;
declare const n: number;
declare const u: string | number;
declare const lit: "a";
export const a = s as number; // error TS2352
export const b = n as string; // error TS2352
export const c = "x" as number; // error TS2352
export const d = 1 as "a"; // error TS2352
export const e = lit as "b";
export const f = lit as string;
export const g = s as "a";
export const h = u as number;
export const i = s as unknown as number;
export const j = s as any;
export const k = n as { a: number }; // error TS2352
export const l = ({ a: 1 }) as string; // error TS2352
export const m = true as number; // error TS2352
export const o = n as number;
export const p = s as {};
export const q = s as { a?: number };
export const r = n as 1;
export const t = [1] as string; // error TS2352
