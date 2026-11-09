function isNumber(x: any): x is number { return typeof x === "number"; }
declare const mixed: (string | number | boolean)[];
export const a: number | undefined = mixed.find(isNumber);
export const b: number[] = mixed.filter(isNumber);
export const c: number | undefined = mixed.find((x): x is number => typeof x === "number");
export const d: (string | number | boolean) | undefined = mixed.find((x) => x === 1);
