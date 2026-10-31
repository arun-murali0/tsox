// Expected error: Promise<string> is not Promise<number>
export {};
async function run<T>(fn: () => Promise<T>): Promise<T> { return fn(); }
const wrong: Promise<number> = run(async () => { return "text"; });
