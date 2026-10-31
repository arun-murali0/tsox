// Expected error: string does not satisfy U = number
export {};
function constrained<U, T extends U>(base: U, value: T): T { return value; }
const wrong = constrained(1, "not a number");
