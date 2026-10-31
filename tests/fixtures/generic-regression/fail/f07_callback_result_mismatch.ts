// Expected error: callback returns string, caller requires number
export {};
function transform<T, U>(value: T, fn: (value: T) => U): U { return fn(value); }
const wrong: number = transform("x", value => { return value.toUpperCase(); });
