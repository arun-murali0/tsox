type ToArray<T> = T extends unknown ? T[] : never;
type Both = ToArray<string | number>;
const good: string[] | number[] = null as unknown as Both;
