type IsString<T> = [T] extends [string] ? true : false;
type Result = IsString<string | number>;
const good: false = null as unknown as Result;
