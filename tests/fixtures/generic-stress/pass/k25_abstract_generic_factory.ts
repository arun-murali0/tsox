type Ctor<T> = new (...a: any[]) => T;
function make<T>(c: Ctor<T>): T { return new c(); }
class A { x = 1; } const a: A = make(A);
export {};
