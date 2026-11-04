type Arg<T> = T extends { f: (x: infer U) => void; g: (x: infer U) => void } ? U : never;
const v: Arg<{ f: (x: { a: 1 }) => void; g: (x: { b: 2 }) => void }> = { a: 1, b: 2 };
export {};
