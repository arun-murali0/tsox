type Mut<T> = { -readonly [K in keyof T]-?: T[K] };
const v: Mut<{ readonly a?: number }> = {};
export {};
