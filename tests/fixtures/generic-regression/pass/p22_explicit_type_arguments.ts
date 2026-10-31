// Explicit generic arguments
export {};
function asPair<A, B>(a: A, b: B): [A, B] { return [a, b]; }
const pair: [string, number] = asPair<string, number>("x", 1);
