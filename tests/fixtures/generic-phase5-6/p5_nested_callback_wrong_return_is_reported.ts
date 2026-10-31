export {};

declare function use<T, U>(value: T, options: { callback: (item: T) => U }): U;
const result: string = use(1, { callback: item => item.toFixed().length });
