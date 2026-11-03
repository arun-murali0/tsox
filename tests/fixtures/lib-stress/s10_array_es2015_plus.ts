// lib.es2015.core / es2016 / es2019 Array additions.
const a = [1, 2, 3];
const f: number | undefined = a.find(x => x > 1);
const fi: number = a.findIndex(x => x > 1);
const inc: boolean = a.includes(2);
const filled: number[] = new Array<number>(3).fill(0);
const of: number[] = Array.of(1, 2, 3);
const flat: number[] = [[1], [2, 3]].flat();
const fm: string[] = a.flatMap(x => [String(x)]);
const cw: number[] = a.copyWithin(0, 1);
const isArr: boolean = Array.isArray(a);
function g(x: unknown) { if (Array.isArray(x)) { const y: any[] = x; } }
const nested: number[][][] = [[[1]]];
const deep: number[] = nested.flat(2);
const rx: ReadonlyArray<number> = a;
const rr: readonly number[] = rx.slice();
const bad1: string = a.find(x => x > 1); // error 2322
const bad2 = a.includes("x"); // error 2345
const bad3: string[] = a.flat(); // error 2322
export {};
