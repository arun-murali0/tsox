declare function tup<T extends unknown[]>(...a: T): T;
const t: [string, number] = tup(1, "a");
export {};
