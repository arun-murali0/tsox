declare function select<T>(value: T, fallback: T): "pair";
declare function select(value: number): "single";

const result: "single" = select(42);
