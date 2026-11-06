declare function lit<const T>(x: T): T;
const v: readonly ["a", 1] = lit(["a", 1]);
export {};
