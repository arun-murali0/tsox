type Wrap<T> = [T] extends [any] ? { v: T } : never;
const x: Wrap<string | number> = { v: "a" } as { v: string };
export {};
