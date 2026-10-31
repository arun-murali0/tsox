type AwaitedLocal<T> = T extends Promise<infer U> ? U : T;
type Value = AwaitedLocal<Promise<number>>;
const good: number = null as unknown as Value;
