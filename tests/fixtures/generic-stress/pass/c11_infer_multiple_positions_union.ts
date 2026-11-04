type Both<T> = T extends { a: infer U; b: infer U } ? U : never;
const v: Both<{ a: string; b: number }> = "s"; const w: Both<{ a: string; b: number }> = 2;
export {};
