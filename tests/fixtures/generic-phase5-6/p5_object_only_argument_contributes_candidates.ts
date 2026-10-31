export {};

declare function wrap<T>(options: { value: T }): T;
const value: number = wrap({ value: 1 });
