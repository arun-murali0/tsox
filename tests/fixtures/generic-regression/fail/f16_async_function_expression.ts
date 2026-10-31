// Expected error: inferred Promise<boolean> is not Promise<string>
export {};
function run<T>(fn: () => Promise<T>): Promise<T> { return fn(); }
const wrong: Promise<string> = run(async function () { return true; });
