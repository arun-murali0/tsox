type Cat<A extends unknown[], B extends unknown[]> = [...A, ...B];
const v: Cat<[1, 2], ["a"]> = [1, 2, "a"];
export {};
