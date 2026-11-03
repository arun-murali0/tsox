// The 14 false errors in lib.es2020.intl: a second `namespace N` block names types from the first.
declare namespace N { interface A { a: number; } }
declare namespace N {
    function f(x: A): A;
    var C: { new (): A };
}
const r: number = N.f({ a: 1 }).a;
const c: N.A = new N.C();
const bad: string = N.f({ a: 1 }).a; // error 2322
export {};
