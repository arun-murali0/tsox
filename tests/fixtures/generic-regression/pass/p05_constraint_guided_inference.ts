// Phase 3: candidates guided by a shared constraint
export {};
interface Base { id: number }
interface Left extends Base { left: string }
interface Right extends Base { right: boolean }
function merge<T extends Base>(a: T, b: T): Base { return a; }
const x: Base = merge({ id: 1, left: "l" }, { id: 2, right: true });
