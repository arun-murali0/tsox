type Tail<T extends unknown[]> = T extends [unknown, ...infer R] ? R : [];
const t: Tail<[1, 2, 3]> = [1, 2, 3];
export {};
