type StringValue<T> = T extends { value: infer U extends string } ? U : never;
type Value = StringValue<{ value: 123 }>;
const bad: Value = 123;
