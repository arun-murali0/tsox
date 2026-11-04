declare function pick<T extends "a" | "b">(x: T): T;
pick("c");
export {};
