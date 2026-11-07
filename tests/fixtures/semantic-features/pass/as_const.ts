const C = { a: 1, b: 2 } as const;
type V = (typeof C)[keyof typeof C];
const v: V = 2;
const arr = ["x", "y"] as const;
type E = (typeof arr)[number];
const e: E = "y";
const one = 1 as const;
const same: 1 = one;
export {};
