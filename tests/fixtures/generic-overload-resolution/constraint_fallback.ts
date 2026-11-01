declare function choose<T extends { id: string }>(value: T): T;
declare function choose(value: number): number;

const result = choose(42);
const checked: number = result;
