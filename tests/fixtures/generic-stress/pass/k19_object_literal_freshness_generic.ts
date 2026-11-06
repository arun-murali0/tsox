declare function id<T>(x: T): T;
const v: { a: number } = id({ a: 1 });
export {};
