type ElementOf<T> = T extends (infer U)[] ? U : never;
type Result = ElementOf<'leaf'[]>;
const good: 'leaf' = null as unknown as Result;
