type StringValue<T> = T extends { value: infer U extends string } ? U : 'fallback';
type Result = StringValue<{ value: 123 }>;
const bad: 123 = null as unknown as Result;
