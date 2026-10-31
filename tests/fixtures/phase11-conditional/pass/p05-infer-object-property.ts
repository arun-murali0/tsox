type ValueOf<T> = T extends { value: infer U } ? U : never;
type Value = ValueOf<{ value: number }>;
const good: number = null as unknown as Value;
