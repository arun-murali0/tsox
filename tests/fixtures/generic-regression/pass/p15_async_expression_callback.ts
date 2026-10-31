// Phase 10: async expression-bodied arrow
export {};
async function run<T>(fn: () => Promise<T>): Promise<T> { return fn(); }
const result: Promise<string> = run(async () => "ok");
