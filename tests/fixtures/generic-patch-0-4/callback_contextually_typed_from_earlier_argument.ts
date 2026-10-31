export {};

declare function use<T, U>(value: T, callback: (item: T) => U): U;
const result: number = use(1, item => item.toFixed().length);
