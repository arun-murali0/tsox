declare function choose<T>(value: T): "generic";
declare function choose(value: number): "number";

const result: "generic" = choose(42);
