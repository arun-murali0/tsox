type O = { a: number; b: string; c: boolean };
const v: O["a" | "b"] = true;
export {};
