interface I { readonly a: number; b: string }
declare const i: I;
i.b = "x";
const n: number = i.a;
class C {
  readonly z: number = 1;
  constructor(public readonly q: number) { this.z = 5; }
}
type M = { readonly [K in "x" | "y"]: number };
declare const m: M;
const sum: number = m.x + m.y;
type W = { -readonly [K in keyof I]: I[K] };
declare const w: W;
w.a = 3;
export {};
