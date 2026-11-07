interface I { readonly a: number; b: string }
declare const i: I;
i.a = 2; // error TS2540
class C {
  readonly z: number = 1;
  constructor(public readonly q: number) { this.z = 5; }
  m() { this.z = 3; } // error TS2540
}
declare const c: C;
c.q = 1; // error TS2540
type M = { readonly [K in "x"]: number };
declare const m: M;
m.x = 1; // error TS2540

class Lit {
  readonly one = 1;
  constructor() { this.one = 5; } // error TS2322
}
export {};
