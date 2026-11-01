declare function choose<T extends string>(left: T, right: T): "text";
declare function choose<U extends number>(left: U, right: U): "numeric";

const result: "numeric" = choose(1, 2);
