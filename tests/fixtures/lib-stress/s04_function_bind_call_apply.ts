// lib.es5 Function.prototype with strictBindCallApply, CallableFunction.
function add(a: number, b: number): number { return a + b; }
const r1: number = add.call(null, 1, 2);
const r2: number = add.apply(null, [1, 2]);
const bound = add.bind(null, 1);
const r3: number = bound(2);
const r4: number = add.bind(null)(1, 2);
class K { v = 1; m(x: string): string { return x + this.v; } }
const k = new K();
const mb: (x: string) => string = k.m.bind(k);
const mc: string = k.m.call(k, "a");
const bad1 = add.call(null, "x", 2); // error 2345
const bad2 = add.apply(null, [1]); // error 2345
const bad3: string = bound(2); // error 2322
export {};
