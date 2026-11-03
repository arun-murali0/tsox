// DOM pattern (695 of 889 DOM warnings): interface + `declare var X: { prototype; new() }`.
interface Foo { x: number; }
declare var Foo: { prototype: Foo; new(x?: number): Foo; };
const a = new Foo(1);
const n: number = a.x;
const bad: string = a.x; // error 2322
export {};
