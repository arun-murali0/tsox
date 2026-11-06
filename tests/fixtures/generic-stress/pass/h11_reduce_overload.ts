const s: number = [1, 2, 3].reduce((a, b) => a + b, 0);
const o: Record<string, number> = [1].reduce((acc, n) => ({ ...acc, [String(n)]: n }), {} as Record<string, number>);
export {};
