// ES5 ReturnType: constrained conditional type with infer.
type RT<T extends (...args: any) => any> = T extends (...args: any) => infer R ? R : any;
declare function mk(): { a: number };
const r: RT<typeof mk> = { a: 1 };
const bad: RT<typeof mk> = { a: "x" }; // error 2322
export {};
