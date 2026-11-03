// 152 DOM + ES5 Pick/Record/Omit: a type parameter constrained by `keyof T`.
declare function get<T, K extends keyof T>(o: T, k: K): T[K];
const n: number = get({ a: 1 }, "a");
const bad: string = get({ a: 1 }, "a"); // error 2322
get({ a: 1 }, "zz"); // error 2345
declare function key<K extends keyof any>(k: K): K;
key("a");
key(1);
key(true); // error 2345
export {};
