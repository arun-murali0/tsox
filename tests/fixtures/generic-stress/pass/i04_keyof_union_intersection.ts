type A = { a: 1; x: 1 }; type B = { b: 1; x: 1 };
const k1: keyof (A | B) = "x"; const k2: keyof (A & B) = "a"; const k3: keyof (A & B) = "b";
export {};
