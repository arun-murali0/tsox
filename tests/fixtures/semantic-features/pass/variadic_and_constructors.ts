declare function last<T extends unknown[]>(...a: [...T, number]): T;
const l1: [string, boolean] = last("a", true, 1);
declare function mid<T extends unknown[]>(...a: [string, ...T, boolean]): T;
const m1: [number, number] = mid("a", 1, 2, true);
declare function bind<A extends unknown[], B extends unknown[], R>(f: (...a: [...A, ...B]) => R, ...a: A): (...b: B) => R;
declare function f3(a: string, b: number, c: boolean): void;
const g = bind(f3, "x");
g(1, true);
type Ctor<T> = new (...a: any[]) => T;
class W { x = 1; }
declare function make<T>(c: Ctor<T>): T;
const w: W = make(W);
function build<T>(c: new (x: number) => T): T { return new c(1); }
export {};
