type ToArray<T> = T extends unknown ? T[] : never;
type Result = ToArray<string | number>;
const bad: Result = (null as unknown as (string | number)[]);
