type BothValues<T> = T extends { left: infer U; right: infer U } ? U : never;
type Values = BothValues<{ left: 'left'; right: 'right' }>;
const good: 'left' | 'right' = null as unknown as Values;
