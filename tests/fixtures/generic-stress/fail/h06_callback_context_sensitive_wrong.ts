declare function app<T, U>(x: T, f: (t: T) => U): U;
const r: string = app("abc", s => s.length);
export {};
