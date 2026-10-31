export {};

declare function use<T, U>(value: T, callback: (item: T) => U): U;
const wrong: Promise<string> = use(1, async item => { return item.toFixed().length; });
