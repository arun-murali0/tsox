const C = { a: 1, b: 2 } as const; type V = (typeof C)[keyof typeof C];
const v: V = 3;
export {};
