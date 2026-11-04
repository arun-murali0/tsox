declare function tup<T extends unknown[]>(...a: T): T;
const t: [number, string] = tup(1, "a");
export {};
