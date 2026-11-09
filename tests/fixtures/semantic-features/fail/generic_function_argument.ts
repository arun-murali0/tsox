function f<T>(value: T) { return value; }
declare function takesNumbers(cb: (x: number) => number): void;
export const bad: (x: number) => string = f; // error TS2322
takesNumbers(f);
