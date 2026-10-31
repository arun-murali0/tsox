type ValueOrArray<T> = T | ValueOrArray<T>[];
const valid: ValueOrArray<number> = [1, [2, 3], [[4]]];
