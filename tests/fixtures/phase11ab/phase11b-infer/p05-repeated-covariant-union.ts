type ReturnOfBoth<T> = T extends { left: () => infer U; right: () => infer U } ? U : never;
type Result = ReturnOfBoth<{ left: () => 'left'; right: () => 'right' }>;
const good: 'left' | 'right' = null as unknown as Result;
