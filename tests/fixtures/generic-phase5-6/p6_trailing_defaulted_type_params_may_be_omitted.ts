export {};

declare function withDefault<T, U = string>(value: T, other?: U): U;
const value: string = withDefault(1);
const defaulted: string = withDefault<number>(1);
const other: number = withDefault<number, number>(1, 2);
