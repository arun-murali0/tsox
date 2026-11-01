declare function choose<T extends string>(value: T): "text";
declare function choose<T extends number>(value: T): "numeric";

const result: "numeric" = choose<number>(42);
