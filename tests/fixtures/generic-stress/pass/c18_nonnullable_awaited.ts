type A = Awaited<Promise<Promise<number>>>; type N = NonNullable<string | null | undefined>;
const a: A = 1; const n: N = "s";
export {};
