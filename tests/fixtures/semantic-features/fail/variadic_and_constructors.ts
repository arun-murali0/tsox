declare function last<T extends unknown[]>(...a: [...T, number]): T;
const l: string = last("a", 1); // error TS2322
declare function bind<A extends unknown[], B extends unknown[], R>(f: (...a: [...A, ...B]) => R, ...a: A): (...b: B) => R;
declare function f3(a: string, b: number, c: boolean): void;
const g = bind(f3, "x");
g("y", true); // error TS2345
type Ctor<T> = new (...a: any[]) => T;
class W { x = 1; }
declare function make<T>(c: Ctor<T>): T;
const s: string = make(W); // error TS2322
export {};
