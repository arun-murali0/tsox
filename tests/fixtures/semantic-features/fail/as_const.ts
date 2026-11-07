const C = { a: 1, b: 2 } as const;
type V = (typeof C)[keyof typeof C];
const v: V = 3; // error TS2322
const arr = ["x", "y"] as const;
type E = (typeof arr)[number];
const e: E = "z"; // error TS2322
C.a = 2; // error TS2540
export {};
