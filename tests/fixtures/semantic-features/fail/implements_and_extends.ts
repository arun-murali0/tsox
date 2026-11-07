interface A { x: number; m(a: number): void }
class Missing implements A { x = 1; } // error TS2420
class Wrong implements A { x = "s"; m(a: number) {} } // error TS2416
class P { x: number = 1; y(): string { return ""; } }
class D extends P { x = "s"; } // error TS2416
interface I1 { x: number }
interface I2 extends I1 { x: string } // error TS2430
export {};
