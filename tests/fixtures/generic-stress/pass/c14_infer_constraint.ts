type First<T> = T extends [infer H extends string, ...any[]] ? H : never;
const v: First<["a", 1]> = "a"; const w: First<[1, 2]> = null as never;
export {};
