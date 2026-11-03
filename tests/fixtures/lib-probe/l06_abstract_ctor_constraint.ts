// ES5 ConstructorParameters / InstanceType: `T extends abstract new (...args: any) => any`.
type CtorParams<T extends abstract new (...args: any) => any> =
    T extends abstract new (...args: infer P) => any ? P : never;
class K1 { constructor(a: number, b: string) {} }
const p: CtorParams<typeof K1> = [1, "x"];
const bad: CtorParams<typeof K1> = ["x", 1]; // error 2322
export {};
