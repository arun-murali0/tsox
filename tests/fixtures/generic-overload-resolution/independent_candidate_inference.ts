declare function choose<T extends string>(value: T): "text";
declare function choose<U extends number>(value: U): "numeric";

const result: "numeric" = choose(42);
