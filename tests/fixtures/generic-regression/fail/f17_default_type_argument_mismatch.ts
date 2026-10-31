// Expected error: number is not assignable to string
export {};
function make<T = string>(value: T = "default" as T): T { return value; }
const wrong: string = make(123);
