export {};

declare function first<T>(ts: T[]): T;
declare function run<R>(f: (c: any) => R): R;
const r: number = run<number>(first);
