function get<T, K extends keyof T>(o: T, k: K): T[K] { return o[k]; }
const n: number = get({ a: 1, b: "s" }, "a"); const s: string = get({ a: 1, b: "s" }, "b");
export {};
