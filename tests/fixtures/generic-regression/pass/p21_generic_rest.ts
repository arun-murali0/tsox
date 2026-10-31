// Generic rest parameter
export {};
function head<T extends unknown[]>(...args: T): T[0] { return args[0]; }
const n: number = head(1, "x", true);
