// ReadonlyArray / readonly tuples / as const with lib.
const ro: readonly number[] = [1, 2, 3];
const m: number[] = ro.map(x => x);
const f: number | undefined = ro.find(x => x > 1);
const t = [1, "a"] as const;
const n: 1 = t[0];
const frozen = Object.freeze([1, 2, 3]);
const l: number = frozen.length;
const tup: [number, string] = [1, "a"];
const [x, y] = tup;
const mm: ReadonlyMap<string, number> = new Map([["a", 1]]);
const ss: ReadonlySet<number> = new Set([1]);
ro.push(4); // error 2339
ro[0] = 1; // error 2542
t[0] = 2; // error 2540
mm.set("b", 2); // error 2339
export {};
