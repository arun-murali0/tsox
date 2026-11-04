function get<T, K extends keyof T>(o: T, k: K): T[K] { return o[k]; }
get({ a: 1 }, "zzz");
export {};
