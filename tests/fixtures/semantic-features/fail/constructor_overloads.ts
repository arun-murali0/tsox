class c1 {
  constructor(a: number);
  constructor(b: string);
  constructor(aorb: any) {}
}
export const x = new c1(10);
export const y = new c1("hello");
function f4(a: number): number;
function f4(b: string): number;
function f4(aOrb: any) { return 1; }
f4("hello"); f4(10);
class D { constructor(a: number); constructor(a: number, b: string); constructor(a: any, b?: any) {} }
export const d1 = new D(1), d2 = new D(1, "x");
export const bad = new c1(true); // error TS2769
export const bad2 = new D(); // error TS2554
class E2 extends c1 {}
export const e1 = new E2(1), e2 = new E2("s");
class G<T> { constructor(a: T); constructor(a: T, b: number); constructor(a: any, b?: any) {} }
export const g1 = new G<string>("x"), g2 = new G(1, 2);
