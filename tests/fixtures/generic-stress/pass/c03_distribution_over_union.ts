type Wrap<T> = T extends any ? { v: T } : never;
const x: Wrap<string | number> = { v: "a" }; const y: Wrap<string | number> = { v: 1 };
export {};
