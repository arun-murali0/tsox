type IsString<T> = T extends string ? 'yes' : 'no';
type Result = IsString<any>;
const good: 'yes' | 'no' = null as unknown as Result;
// @ts-expect-error any conditional is represented by both branches, not only yes
const bad: 'yes' = null as unknown as Result;
