type PropOf<T> = T extends { x: infer X } ? X : never;
const v: PropOf<{ x: string; y: number }> = "s";
export {};
