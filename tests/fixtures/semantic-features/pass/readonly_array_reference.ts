class A { a = 1 }
declare let xs: A[];
const r: ReadonlyArray<A> = xs;
const e: ReadonlyArray<number> = [];
const r2: readonly A[] = xs;
export { r, e, r2 };
