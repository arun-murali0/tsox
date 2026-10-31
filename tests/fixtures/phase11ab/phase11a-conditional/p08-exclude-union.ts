type ExcludeLocal<T, U> = T extends U ? never : T;
type Result = ExcludeLocal<'a' | 'b' | 'c', 'a' | 'c'>;
const good: 'b' = null as unknown as Result;
// @ts-expect-error filtered union members must disappear
const bad: Result = 'a';
