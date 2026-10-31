// Phase 10: block-bodied callback return inference
export {};
function transform<T, U>(value: T, fn: (value: T) => U): U { return fn(value); }
const result: number = transform("abcd", value => { return value.length; });
