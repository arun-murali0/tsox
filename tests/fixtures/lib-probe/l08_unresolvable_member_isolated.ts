// One hard member must not make the rest of the interface unusable.
interface Big {
    ok: number;
    hard<K extends keyof Big>(k: K): Big[K];
}
declare const big: Big;
const n: number = big.ok;
const bad: string = big.ok; // error 2322
export {};
