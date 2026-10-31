export {};

declare function id<T>(x: T): T;
declare function apply(f: (x: number) => number): number;
apply(id);
