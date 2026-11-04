type Last<F> = F extends { (...a: any[]): infer R } ? R : never;
declare const o: { (a: string): string; (a: number): number };
const r: Last<typeof o> = 1;
export {};
