declare function id<T>(x: T): T;
declare function map<A, B>(f: (a: A) => B): (xs: A[]) => B[];
const m = map(id); const r: string[] = m(["a"]);
export {};
