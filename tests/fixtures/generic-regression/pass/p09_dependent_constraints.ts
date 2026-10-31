// Phase 7: dependent generic constraints
export {};
function constrained<U, T extends U>(base: U, value: T): U { return base; }
const result: { id: number } = constrained({ id: 1 }, { id: 2, extra: true });
