type Both<T> = T extends { produce: () => infer U; consume: (value: infer U) => void } ? U : never;
type Result = Both<{ produce: () => 'produced'; consume: (value: string) => void }>;
const good: 'produced' = null as unknown as Result;
