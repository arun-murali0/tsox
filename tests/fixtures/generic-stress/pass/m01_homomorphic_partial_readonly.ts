type P<T> = { [K in keyof T]?: T[K] }; type R<T> = { readonly [K in keyof T]: T[K] };
const a: P<{ x: number; y: string }> = { x: 1 }; const b: R<{ x: number }> = { x: 1 };
export {};
