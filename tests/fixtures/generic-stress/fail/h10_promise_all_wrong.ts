declare const a: Promise<number>; declare const b: Promise<string>;
async function f() { const [x, y] = await Promise.all([a, b]); const n: string = x; }
export {};
