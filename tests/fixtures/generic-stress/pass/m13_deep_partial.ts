type DP<T> = { [K in keyof T]?: T[K] extends object ? DP<T[K]> : T[K] };
const v: DP<{ a: { b: { c: number } }; d: string }> = { a: { b: {} } };
export {};
