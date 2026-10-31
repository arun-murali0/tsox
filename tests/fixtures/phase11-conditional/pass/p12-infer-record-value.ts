type RecordValue<T> = T extends Record<string, infer U> ? U : never;
type Value = RecordValue<Record<string, number>>;
const good: number = null as unknown as Value;
