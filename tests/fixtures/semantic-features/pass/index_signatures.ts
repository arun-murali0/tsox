interface Dict { [k: string]: number; n: number }
interface Indexed { [i: number]: string }
declare const d: Dict;
declare const ix: Indexed;
const a: number | undefined = d["anything"];
const b: number = d.n;
const c: string | undefined = ix[0];
declare function values<T>(x: { [k: string]: T }): T;
const v: number = values({ a: 1, b: 2 });
const lit: Dict = { n: 1, other: 2 };
type Keys = keyof Dict;
const k: Keys = "whatever";
export {};
declare const rec: Record<string, number>;
export const recText: string = rec.toString();
export const recOwn: boolean = rec.hasOwnProperty("k");
