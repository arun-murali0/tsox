declare const words: string[];
export const lengths: string[] = words.map((w) => w.length); // error TS2322
export const kept: number[] = words.filter((w) => w.length > 1); // error TS2322
export const text: number = String(1); // error TS2322
declare const a: Promise<number>;
declare const b: Promise<string>;
export async function both() {
  const [x] = await Promise.all([a, b]);
  const n: string = x; // error TS2322
  return n;
}
export const resolved: Promise<string> = Promise.resolve(1); // error TS2322
type Settled<T> = { [K in keyof T]: Promise<T[K]> };
export const tuple: Settled<[string, number]> = [a, a]; // error TS2322
type Unwrapped = Awaited<Promise<Promise<number>>>;
export const unwrapped: Unwrapped = "s"; // error TS2322
