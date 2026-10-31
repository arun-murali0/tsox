export {};

declare function f<T>(a: T, o: { v: T }): T;
f("a", { v: 1 });
