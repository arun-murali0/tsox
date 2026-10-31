// Phase 9: ordinary argument provides inference evidence
export {};
function apply<T>(seed: T, fn: (value: T) => T): T { return fn(seed); }
const result: string = apply("seed", value => value.toUpperCase());
