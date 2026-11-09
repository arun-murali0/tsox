interface P { readonly n: number }
declare const p: P;
p.n++; // error TS2540
--p.n; // error TS2540
p.n += 1; // error TS2540
export class C {
  readonly v: number = 1;
  constructor() {
    this.v = 2;
    const f = () => { this.v = 3; }; // error TS2540
    f();
  }
  m() {
    this.v++; // error TS2540
  }
}
