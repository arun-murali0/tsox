// Phase 6: omitted trailing defaults
export {};
function pair<A, B = A>(a: A, b?: B): [A, B | undefined] { return [a, b]; }
const p = pair(1);
const first: number = p[0];
