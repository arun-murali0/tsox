type IsStr<T> = T extends string ? true : false;
function f<T>(x: T): IsStr<T> { return null as any; }
const r: true = f(1);
export {};
