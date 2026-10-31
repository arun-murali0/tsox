type ToArray<T> = T extends unknown ? T[] : never;
type Result = ToArray<string | number>;
const good: string[] | number[] = null as unknown as Result;
// @ts-expect-error distribution must not produce an array of the whole union
const bad: Result = null as unknown as (string | number)[];
