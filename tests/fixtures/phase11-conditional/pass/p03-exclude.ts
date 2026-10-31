type ExcludeLocal<T, U> = T extends U ? never : T;
type OnlyB = ExcludeLocal<'a' | 'b', 'a'>;
const good: 'b' = null as unknown as OnlyB;
