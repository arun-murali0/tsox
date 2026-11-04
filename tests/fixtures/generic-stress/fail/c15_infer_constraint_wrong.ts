type First<T> = T extends [infer H extends string, ...any[]] ? H : "none";
const v: First<[1, 2]> = "a";
export {};
