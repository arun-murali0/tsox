type ValueOf<T> = T extends { value: infer U } ? U : never;
type Result = ValueOf<{ value: 42 }>;
const bad: string = null as unknown as Result;
