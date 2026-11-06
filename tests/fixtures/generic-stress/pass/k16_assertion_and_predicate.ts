function isStr<T>(x: T): x is T & string { return typeof x === "string"; }
function g(x: number | string) { if (isStr(x)) { const s: string = x; } }
export {};
