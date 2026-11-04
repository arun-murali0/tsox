type Kind<T> = T extends string ? "s" : T extends number ? "n" : "o";
const a: Kind<number> = "s";
export {};
