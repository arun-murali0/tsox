type Split<S extends string, D extends string> = S extends `${infer H}${D}${infer T}` ? [H, ...Split<T, D>] : [S];
const v: Split<"a.b.c", "."> = ["a", "b"];
export {};
