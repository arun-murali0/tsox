export {};

declare function pick<T extends U, U>(value: T, bound: U): T;
pick(1, "x");
