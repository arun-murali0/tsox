type Pk<T, K extends keyof T> = { [P in K]: T[P] };
type Om<T, K extends keyof any> = Pk<T, Exclude<keyof T, K>>;
const v: Om<{ a: 1; b: 2; c: 3 }, "b"> = { a: 1, b: 2, c: 3 };
export {};
