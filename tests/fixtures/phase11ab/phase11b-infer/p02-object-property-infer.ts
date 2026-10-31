type ValueOf<T> = T extends { value: infer U } ? U : never;
type Result = ValueOf<{ value: 42 }>;
const good: 42 = null as unknown as Result;
