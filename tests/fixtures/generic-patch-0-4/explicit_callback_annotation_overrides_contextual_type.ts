export {};

declare function use<T, U>(value: T, callback: (item: T) => U): U;
use(1, (item: string) => item.length);
