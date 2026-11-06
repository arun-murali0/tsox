type Rec<K extends string, V> = { [P in K]: V };
const v: Rec<"a" | "b", number> = { a: 1, b: 2 };
export {};
