type IsString<T> = T extends string ? true : false;
type Result = IsString<unknown>;
const good: false = null as unknown as Result;
