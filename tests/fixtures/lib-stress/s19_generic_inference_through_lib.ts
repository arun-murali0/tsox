// Inference through lib signatures: tuples, entries, reducers, guards.
const pairs = Object.entries({ a: 1, b: 2 }).map(([k, v]) => `${k}=${v}`);
const lens: number[] = ["a", "bb"].map(s => s.length);
const byLen: Record<number, string[]> = ["a", "bb", "cc"].reduce((acc: Record<number, string[]>, s) => {
  (acc[s.length] ||= []).push(s);
  return acc;
}, {});
const strsOnly: string[] = [1, "a", 2, "b"].filter((x): x is string => typeof x === "string");
const truthy: string[] = ["a", "", "b"].filter(Boolean);
const maxes: number = Math.max(...[1, 2, 3].map(x => x * 2));
const m = new Map(Object.entries({ a: 1 }));
const mv: number | undefined = m.get("a");
const sortedEntries: [string, number][] = [...m.entries()].sort((a, b) => b[1] - a[1]);
const idx = Object.fromEntries(["x", "y"].map((k, i) => [k, i] as const));
const iv: number = idx["x"];
const tup = Promise.all([1, "a", true] as const);
const flatMapped: string[] = [[1, 2], [3]].flatMap(a => a.map(String));
const zipped: [number, string][] = [1, 2].map((n, i) => [n, ["a", "b"][i]] as [number, string]);
const grouped = new Map<string, number[]>();
for (const [k, v] of [["a", 1], ["b", 2]] as [string, number][]) { grouped.set(k, [...(grouped.get(k) ?? []), v]); }
const bad1: number[] = pairs; // error 2322
const bad2: string[] = [1, "a"].filter((x): x is number => typeof x === "number"); // error 2322
const bad3: string = Math.max(...[1, 2]); // error 2322
export {};
