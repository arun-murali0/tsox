// lib.es2015.collection / iterable: Map, Set, WeakMap, WeakSet.
const m = new Map<string, number>([["a", 1], ["b", 2]]);
m.set("c", 3).set("d", 4);
const v: number | undefined = m.get("a");
const has: boolean = m.has("a");
const size: number = m.size;
for (const [k, val] of m) { const a: string = k; const b: number = val; }
const ks: string[] = [...m.keys()];
const es: [string, number][] = Array.from(m.entries());
m.forEach((val, key, map) => { const x: Map<string, number> = map; });
const s = new Set<number>([1, 2, 3]);
s.add(4);
const sv: number[] = [...s];
const wm = new WeakMap<object, string>();
wm.set({}, "x");
const ws = new WeakSet<object>();
ws.add({});
const inferred = new Map([[1, "one"], [2, "two"]]);
const iv: string | undefined = inferred.get(1);
m.set(1, 1); // error 2345
const bad: string = m.get("a"); // error 2322
s.add("x"); // error 2345
wm.set("k", "v"); // error 2345
export {};
