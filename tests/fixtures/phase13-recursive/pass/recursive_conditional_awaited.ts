type RecursiveAwaited<T> = T extends Promise<infer Item> ? RecursiveAwaited<Item> : T;
type Result = RecursiveAwaited<Promise<Promise<Promise<number>>>>;
const value: number = null as unknown as Result;
