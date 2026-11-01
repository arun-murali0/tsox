type A = { a: 1 };
type B = { b: 2 };
const one: keyof (A & B) = 'a';
const two: keyof (A & B) = 'b';
const bad: keyof (A & B) = 'c';
