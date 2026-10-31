type RecursiveAwaited<T> = T extends Promise<infer Item> ? RecursiveAwaited<Item> : T;
type Result = RecursiveAwaited<Promise<Promise<number>>>;
const value: string = null as unknown as Result;
