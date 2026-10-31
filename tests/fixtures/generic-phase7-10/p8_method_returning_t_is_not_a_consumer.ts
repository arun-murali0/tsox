export {};

declare function make<T>(o: { build(): T }, fallback: T): T;
const r: string = make({ build() { return "a"; } }, "b");
