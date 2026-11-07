interface A { x: number; y?: string; m(a: number): void }
class Ok implements A { x = 1; m(a: number, b?: string) {} extra = true; }
interface Box<T> { get(): T }
class B2<T> implements Box<T> { constructor(private v: T) {} get() { return this.v; } }
class P { x = 1; m(a: number) { return a; } }
class Q extends P { y = 2; m(a: number) { return a + 1; } }
class Gen<T> { v!: T }
class GS extends Gen<string> { z = 1 }
interface I1 { x: number }
interface I2 extends I1 { y: string }
interface G<T> { v: T }
interface GN extends G<number> { w: number }
export {};
