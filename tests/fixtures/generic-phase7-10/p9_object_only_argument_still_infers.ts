export {};

declare function wrap<T>(o: { value: T }): T;
const r: number = wrap({ value: 1 });
