type O = { a: number; b: string; c: boolean };
const v: O["a" | "b"] = 1; const w: O["a" | "b"] = "s"; const z: O[keyof O] = true;
export {};
