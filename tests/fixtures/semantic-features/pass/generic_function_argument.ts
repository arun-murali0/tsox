function f<T>(value: T) { return value; }
function h<R>(func: (x: number) => R): R { return null as any; }
export var z1: number = h<number>(f);
export var z2: number = h(f);
declare function apply(cb: (x: string) => string): void;
apply(f);
declare const nums: number[];
export const m = nums.map(f);
export const idn: (x: number) => number = f;
