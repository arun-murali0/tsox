declare function either<T>(a: T, b: T): T;
const v: string | number = either("a", 1 as number | string);
export {};
