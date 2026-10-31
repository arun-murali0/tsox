type ReturnOf<T> = T extends () => infer U ? U : never;
type Result = ReturnOf<() => 'result'>;
const good: 'result' = null as unknown as Result;
