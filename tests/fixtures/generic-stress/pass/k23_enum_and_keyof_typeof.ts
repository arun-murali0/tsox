const C = { a: 1, b: 2 } as const; type K = keyof typeof C; type V = (typeof C)[K];
const k: K = "a"; const v: V = 2;
export {};
