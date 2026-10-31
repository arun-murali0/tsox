export {};

declare function len<T extends string>(x: T): number;
const f: (x: number) => number = len;
