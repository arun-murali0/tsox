type ToArray<T> = T extends unknown ? T[] : never;
type Result = ToArray<never>;
// @ts-expect-error distributive conditional over never is never
const bad: Result = [];
