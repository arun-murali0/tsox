type ToArray<T> = T extends unknown ? T[] : never;
type Empty = ToArray<never>;
const bad: Empty = 123;
