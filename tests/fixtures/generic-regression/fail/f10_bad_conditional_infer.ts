// Expected error: ElementOf is string but a number is assigned
export {};
type ElementOf<T> = T extends readonly (infer U)[] ? U : never;
const wrong: ElementOf<readonly string[]> = 42;
