declare const ps: Promise<number>[];
export async function f() {
  const a: string = await Promise.all(ps); // error TS2322
  const b: string = await Promise.race(ps); // error TS2322
  return [a, b];
}
export namespace ns { export interface I { a: number; b: string } }
export class K implements ns.I { a = "x"; b = 1 } // error TS2416
