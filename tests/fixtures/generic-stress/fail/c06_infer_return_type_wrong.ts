type Ret<F> = F extends (...a: any[]) => infer R ? R : never;
declare function mk(): { id: number };
const r: Ret<typeof mk> = { id: "x" };
export {};
