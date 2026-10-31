export {};

declare function use<T, U>(value: T, callback: (item: T) => U): U;
const ok: number = use(1, item => { return item.toFixed().length; });
