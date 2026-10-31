export {};

declare function f<T>(a: T, o: { v: T }): T;
const r: string = f("a", { v: "b" });
