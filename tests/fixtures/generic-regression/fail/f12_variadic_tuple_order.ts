// Expected error: tuple positions are wrong
export {};
function concat<A extends unknown[], B extends unknown[]>(a: [...A], b: [...B]): [...A, ...B] { return [...a, ...b]; }
const wrong: [string, number, boolean] = concat([1, "x"], [true]);
