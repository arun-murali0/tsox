// Phase 10: async block-bodied arrow
export {};
async function run<T>(fn: () => Promise<T>): Promise<T> { return fn(); }
const result: Promise<number> = run(async () => { return 7; });
