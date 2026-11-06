declare function f(x: string): string; declare function f<T>(x: T[]): T;
const b: string = f([1]);
export {};
