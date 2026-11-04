interface Cmp<T extends Cmp<T>> { cmp(o: T): number; }
class N implements Cmp<N> { cmp(o: string) { return 0; } }
export {};
