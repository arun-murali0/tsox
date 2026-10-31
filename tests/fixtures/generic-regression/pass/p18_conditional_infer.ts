// Conditional type inference
export {};
type ElementOf<T> = T extends readonly (infer U)[] ? U : never;
const item: string = null as unknown as ElementOf<readonly string[]>;
