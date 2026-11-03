// What the lib's Array<T>.map signature must give: element-typed callbacks and a typed result.
const xs: number[] = [1, 2, 3];
const strs: string[] = xs.map(x => String(x));
const bad: number[] = xs.map(x => String(x)); // error 2322
xs.forEach(x => { const s: string = x; }); // error 2322
export {};
