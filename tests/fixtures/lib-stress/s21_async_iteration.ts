// lib.es2018.asynciterable / asyncgenerator.
async function* agen(): AsyncGenerator<number, void, undefined> { yield 1; yield 2; }
async function use() {
  for await (const n of agen()) { const x: number = n; }
  const it: AsyncIterator<number> = agen();
  const r: IteratorResult<number, void> = await it.next();
}
const ai: AsyncIterable<string> = { async *[Symbol.asyncIterator]() { yield "a"; } };
async function use2() { for await (const s of ai) { const t: string = s; } }
async function use3() { for await (const v of [Promise.resolve(1), 2]) { const n: number = v; } }
async function bad() { for await (const n of agen()) { const s: string = n; } } // error 2322
export {};
