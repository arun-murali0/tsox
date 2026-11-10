declare function assert(value: any): asserts value;
declare const o: { fn(): number; tag: number };
const a = (0, o.fn)();
const b = (0, o["fn"])();
declare function pick(param: number | null | undefined): number | null;
function foo(param: number | null | undefined): number | null {
  const val = param !== undefined;
  return val ? (assert(param !== undefined), param) : null;
}
export { a, b, pick, foo };
