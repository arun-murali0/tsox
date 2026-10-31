function first<T, U>(p: [T, U]): T { return p[0]; }
const n: number = first([1, "x"]);
const s: string = first([1, "x"]);
