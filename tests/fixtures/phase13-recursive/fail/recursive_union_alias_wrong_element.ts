type ValueOrArray<T> = T | ValueOrArray<T>[];
const invalid: ValueOrArray<number> = [1, [2, 'bad']];
