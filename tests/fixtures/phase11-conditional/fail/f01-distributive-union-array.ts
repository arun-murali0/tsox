type ToArray<T> = T extends unknown ? T[] : never;
type Both = ToArray<string | number>;
const bad: Both = null as unknown as (string | number)[];
