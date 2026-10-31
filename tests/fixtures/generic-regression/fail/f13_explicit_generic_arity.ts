// Expected error: too many type arguments
export {};
function identity<T>(value: T): T { return value; }
identity<string, number>("x");
