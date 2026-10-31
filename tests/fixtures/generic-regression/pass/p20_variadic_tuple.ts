// Variadic tuple inference
export {};
function concat<A extends unknown[], B extends unknown[]>(a: [...A], b: [...B]): [...A, ...B] { return [...a, ...b]; }
const result: [number, string, boolean] = concat([1, "x"], [true]);
