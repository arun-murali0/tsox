type Classify<T> = T extends string ? (T extends 'x' ? 1 : 2) : 3;
const a: 1 = null as unknown as Classify<'x'>;
const b: 2 = null as unknown as Classify<'y'>;
const c: 3 = null as unknown as Classify<number>;
