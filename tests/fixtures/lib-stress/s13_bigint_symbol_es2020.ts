// lib.es2020.bigint / symbol-wellknown / intl pieces in the default set.
const a: bigint = BigInt(10);
const b: bigint = 10n ** 2n;
const c: string = a.toString(16);
const d: bigint = BigInt.asUintN(8, 257n);
const arr = new BigUint64Array(2);
const e: bigint = arr[0] + 1n;
const f: string = a.toLocaleString("en-US");
const s: symbol = Symbol.for("x");
const k: string | undefined = Symbol.keyFor(s);
const bad1: number = a; // error 2322
const bad2 = a + 1; // error 2365
const bad3 = BigInt("x", 2); // error 2554
export {};
