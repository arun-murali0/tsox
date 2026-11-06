type Flat<T> = T extends readonly (infer U)[] ? Flat<U> : T;
const a: Flat<number[][][]> = "s";
export {};
