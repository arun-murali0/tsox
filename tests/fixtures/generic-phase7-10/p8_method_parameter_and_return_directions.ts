export {};

declare function conv<T>(o: { convert(x: T): T }): T;
const r: number = conv({ convert(x: number) { return x + 1; } });
