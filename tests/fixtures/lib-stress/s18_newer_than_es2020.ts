// Newer lib members under the default ES2020 set: tsc reports each one.
const a = [1, 2, 3].at(0); // error 2550
const b = Object.hasOwn({}, "a"); // error 2550
const c = "a-b".replaceAll("-", "+"); // error 2550
const d = Promise.any([Promise.resolve(1)]); // error 2550
const e = new Error("x", { cause: "y" }); // error 2554
const f = [1, 2, 3].findLast(x => x > 1); // error 2550
const g = new WeakRef({}); // error 2304
const h = structuredClone(1);
const j = new AggregateError([], "m"); // error 2304
const i = [3, 1].toSorted(); // error 2550
export {};
