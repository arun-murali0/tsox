declare function bind<A extends unknown[], B extends unknown[], R>(f: (...a: [...A, ...B]) => R, ...a: A): (...b: B) => R;
declare function f3(a: string, b: number, c: boolean): void;
const g = bind(f3, "x"); g(1, true);
export {};
