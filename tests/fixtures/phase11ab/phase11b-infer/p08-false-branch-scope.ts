type Select<U, T> = T extends { value: infer U } ? U : U;
type Result = Select<boolean, { other: 1 }>;
const good: boolean = null as unknown as Result;
