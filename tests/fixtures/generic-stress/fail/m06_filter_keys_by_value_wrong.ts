type OnlyStr<T> = { [K in keyof T as T[K] extends string ? K : never]: T[K] };
const v: OnlyStr<{ a: string; b: number }> = { a: "x", b: 1 };
export {};
