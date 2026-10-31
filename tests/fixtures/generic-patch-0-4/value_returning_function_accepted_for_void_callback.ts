export {};

declare function returnsNumber(): number;
declare function consume(callback: () => void): void;
consume(returnsNumber);
const callback: () => void = () => 123;
