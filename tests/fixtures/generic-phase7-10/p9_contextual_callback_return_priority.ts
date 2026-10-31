export {};

declare function map<T, U>(xs: T[], f: (x: T) => U): U[];
const r: string[] = map([1, 2], x => x * 2);
