type StringValue<T> = T extends { value: infer U extends string } ? U : never;
type Result = StringValue<{ value: 'ok' }>;
const good: 'ok' = null as unknown as Result;
