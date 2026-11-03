// lib.es2015.iterable / generator / symbol: iteration protocols.
function* gen(): Generator<number, string, boolean> { const b: boolean = yield 1; return "done"; }
const it = gen();
const r: IteratorResult<number, string> = it.next(true);
const arr: number[] = Array.from({ length: 3 }, (_, i) => i);
const chars: string[] = [..."abc"];
const fromSet: number[] = Array.from(new Set([1, 2]));
const iterable: Iterable<number> = [1, 2, 3];
for (const x of iterable) { const n: number = x; }
const entries: IterableIterator<[number, string]> = ["a", "b"].entries();
class Range implements Iterable<number> {
  constructor(private n: number) {}
  *[Symbol.iterator](): Iterator<number> { for (let i = 0; i < this.n; i++) yield i; }
}
const spread: number[] = [...new Range(3)];
const sym: symbol = Symbol("s");
const desc: string | undefined = sym.description;
const o = { [Symbol.toStringTag]: "T" };
const [a, ...rest] = new Set([1, 2, 3]);
const bad1: string[] = [...new Range(3)]; // error 2322
const bad2: Iterable<string> = [1, 2]; // error 2322
export {};
