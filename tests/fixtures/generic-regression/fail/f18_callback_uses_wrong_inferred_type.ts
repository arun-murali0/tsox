// Expected error: inferred T is string from seed
export {};
function apply<T>(seed: T, fn: (value: T) => T): T { return fn(seed); }
apply("seed", value => value.toFixed(2));
