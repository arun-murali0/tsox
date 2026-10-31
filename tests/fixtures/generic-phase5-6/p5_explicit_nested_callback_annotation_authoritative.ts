export {};

declare function use<T, U>(value: T, options: { callback: (item: T) => U }): U;
use(1, { callback: (item: string) => item.length });
