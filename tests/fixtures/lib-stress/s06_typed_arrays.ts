// lib.es5/es2017 typed arrays, ArrayBuffer, DataView.
const buf = new ArrayBuffer(16);
const u8 = new Uint8Array(buf);
const f64 = new Float64Array([1, 2, 3]);
const i32 = Int32Array.from([1, 2, 3], x => x * 2);
const dv = new DataView(buf, 0, 8);
dv.setUint16(0, 258, true);
const g: number = dv.getUint16(0, true);
u8[0] = 255;
const len: number = u8.length + f64.byteLength + buf.byteLength;
const sub: Uint8Array = u8.subarray(1, 4);
const mapped: Float64Array = f64.map(x => x * 2);
const idx: number = f64.indexOf(2);
u8.set([1, 2, 3], 4);
const big = new BigInt64Array(2);
const bv: bigint = big[0];
const copy: ArrayBuffer = buf.slice(0, 4);
const bad1: string = u8[0]; // error 2322
const bad2 = new Uint8Array("x"); // error 2769
u8.fill("a"); // error 2345
export {};
