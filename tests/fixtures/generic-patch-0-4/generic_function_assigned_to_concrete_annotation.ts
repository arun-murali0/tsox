export {};

declare function id<T>(x: T): T;
const g: (x: number) => number = id;
