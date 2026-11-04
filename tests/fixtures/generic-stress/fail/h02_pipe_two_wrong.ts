declare function pipe<A, B, C>(f: (a: A) => B, g: (b: B) => C): (a: A) => C;
const p = pipe((n: number) => String(n), s => s.toFixed());
export {};
