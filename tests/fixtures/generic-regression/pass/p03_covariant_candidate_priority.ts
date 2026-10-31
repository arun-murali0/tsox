// Phase 2: covariant evidence
export {};
function choose<T>(a: T, b: T): T { return a; }
const result: string | number = choose("x", 1);
