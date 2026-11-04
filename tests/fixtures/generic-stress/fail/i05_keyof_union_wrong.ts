type A = { a: 1; x: 1 }; type B = { b: 1; x: 1 };
const k1: keyof (A | B) = "a";
export {};
