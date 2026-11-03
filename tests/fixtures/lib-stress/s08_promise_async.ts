// lib.es2015.promise / es2018 finally / es2020 allSettled; async functions.
const p1: Promise<number> = Promise.resolve(1);
const p2: Promise<string> = p1.then(n => String(n));
const p3: Promise<number> = p1.catch(() => 0);
const p4: Promise<number> = p1.finally(() => {});
const all: Promise<[number, string]> = Promise.all([p1, p2]);
const race: Promise<number | string> = Promise.race([p1, p2]);
const settled: Promise<PromiseSettledResult<number>[]> = Promise.allSettled([p1]);
const rej: Promise<never> = Promise.reject(new Error("x"));
const created = new Promise<number>((resolve, reject) => { resolve(1); reject("e"); });
async function f(): Promise<number> { const n = await p1; return n + 1; }
async function g() { const [a, b] = await Promise.all([p1, p2]); return a + b.length; }
const gr: Promise<number> = g();
async function h(): Promise<string> { return await p1.then(String); }
const aw: Promise<number> = (async () => 1)();
async function bad1(): Promise<string> { return 1; } // error 2322
const bad2: Promise<string> = p1; // error 2322
const bad3 = new Promise<number>(resolve => resolve("x")); // error 2345
export {};
