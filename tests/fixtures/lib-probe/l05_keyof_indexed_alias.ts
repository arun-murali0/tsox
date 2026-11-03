// 51 typed-array interfaces: `type ArrayBufferLike = ArrayBufferTypes[keyof ArrayBufferTypes]`.
interface BufA { kind: "a"; }
interface BufB { kind: "b"; }
interface BufTypes { A: BufA; B: BufB; }
type BufLike = BufTypes[keyof BufTypes];
interface View<TB extends BufLike = BufLike> { buffer: TB; }
declare const v: View;
const k: "a" | "b" = v.buffer.kind;
const bad: "c" = v.buffer.kind; // error 2322
export {};
