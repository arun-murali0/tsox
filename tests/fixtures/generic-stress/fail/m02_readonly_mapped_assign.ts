type R<T> = { readonly [K in keyof T]: T[K] };
const b: R<{ x: number }> = { x: 1 }; b.x = 2;
export {};
