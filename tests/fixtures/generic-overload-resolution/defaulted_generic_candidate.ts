declare function select<T = string>(value: string): "text";
declare function select(value: number): "numeric";

const result: "numeric" = select(42);
