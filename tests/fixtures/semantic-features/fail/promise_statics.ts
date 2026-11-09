declare const a: Promise<number>;
declare const b: Promise<string>;
export const p1: Promise<string> = new Promise<number>((resolve) => resolve(1)); // error TS2322
export const p2: Promise<number> = new Promise<number>((resolve) => resolve(1));
export async function f() {
  const r = await Promise.allSettled([a, b]);
  const n: number = r; // error TS2322
  return n;
}
declare const rec: Record<string, number>;
export const viaDot = rec.total; // error TS4111
export const viaBracket = rec["total"];
