// Phase 5: deferred object literal evidence
export {};
interface Config<T> { value: T; convert: (value: T) => string }
function configure<T>(seed: T, config: Config<T>): Config<T> { return config; }
const cfg = configure(3, { value: 5, convert: n => String(n) });
const value: number = cfg.convert(cfg.value) as unknown as number;
