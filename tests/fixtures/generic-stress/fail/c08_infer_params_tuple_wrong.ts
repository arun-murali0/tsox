type Params<F> = F extends (...a: infer P) => any ? P : never;
declare function g(a: string, b: number): void;
const t: Params<typeof g> = [1, "x"];
export {};
