declare function f(x: string): string; declare function f<T>(x: T[]): T;
const a: string = f("s"); const b: number = f([1]);
export {};
