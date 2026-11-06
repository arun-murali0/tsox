declare function pick<T extends "a" | "b">(x: T): T;
const v: "a" = pick("a");
export {};
