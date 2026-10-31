// Expected error: inferred identity result is number, not string
export {};
function identity<T>(value: T): T { return value; }
const wrong: string = identity(42);
