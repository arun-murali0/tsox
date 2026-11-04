type U2I<U> = (U extends any ? (k: U) => void : never) extends (k: infer I) => void ? I : never;
const v: U2I<{ a: 1 } | { b: 2 }> = { a: 1 };
export {};
