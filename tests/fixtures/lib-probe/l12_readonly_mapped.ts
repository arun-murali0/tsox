// ES5 Readonly<T>: homomorphic mapped type with a modifier.
type RO<T> = { readonly [P in keyof T]: T[P] };
declare const o: RO<{ a: number }>;
const n: number = o.a;
o.a = 2; // error 2540
export {};
