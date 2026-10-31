// Phase 1: identity inference
export {};
function identity<T>(value: T): T { return value; }
const n: number = identity(42);
const s: string = identity("ok");
