const o = { a: 1, b: "s" } satisfies Record<string, number | string>;
const n: number = o.a;
export {};
