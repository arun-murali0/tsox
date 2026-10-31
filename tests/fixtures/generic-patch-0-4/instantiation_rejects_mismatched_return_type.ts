export {};

declare function id<T>(x: T): T;
const bad: (x: number) => string = id;
