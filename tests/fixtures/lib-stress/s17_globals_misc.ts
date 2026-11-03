// Global functions and values.
const a: string = encodeURIComponent("a b") + decodeURIComponent("%20") + escape("x");
const b: number = parseInt("1") + parseFloat("1.5");
const c: boolean = isFinite(1) && !isNaN(2);
const d: typeof globalThis = globalThis;
const e: Date = new Date(2020, 1, 1);
const f: number = Date.now() + e.getTime() + e.getFullYear();
const g: string = e.toISOString() + e.toLocaleDateString("en-US");
const h: Error = new Error("m");
const i: string = h.message + (h.stack ?? "");
class MyErr extends Error { code = 1; constructor(m: string) { super(m); this.name = "MyErr"; } }
const j: number = new MyErr("x").code;
try { throw new TypeError("t"); } catch (err) { if (err instanceof Error) { const m: string = err.message; } }
const k: RangeError = new RangeError("r");
const bad1: string = Date.now(); // error 2322
const bad2 = new Date("a", "b", "c"); // error 2345
export {};
