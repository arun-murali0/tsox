type Head<T extends unknown[]> = T extends [infer H, ...unknown[]] ? H : never;
type Tail<T extends unknown[]> = T extends [unknown, ...infer R] ? R : [];
const h: Head<[1, 2, 3]> = 1; const t: Tail<[1, 2, 3]> = [2, 3];
export {};
