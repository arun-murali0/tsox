type Kind<T> = T extends string ? "s" : T extends number ? "n" : T extends boolean ? "b" : "o";
const a: Kind<string> = "s"; const b: Kind<42> = "n"; const c: Kind<true> = "b"; const d: Kind<{}> = "o";
export {};
