declare const mixed: (string | number)[];
function isNumber(x: unknown): x is number { return typeof x === "number"; }
export const s: string | undefined = mixed.find(isNumber); // error TS2322
export const t: string[] = mixed.filter(isNumber); // error TS2322
