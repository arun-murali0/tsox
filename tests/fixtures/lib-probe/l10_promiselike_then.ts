// ES5 PromiseLike.then: generics with defaults, union with itself, optional nullable callbacks.
interface PL<T> {
    then<R1 = T, R2 = never>(
        ok?: ((v: T) => R1 | PL<R1>) | null,
        no?: ((r: any) => R2 | PL<R2>) | null,
    ): PL<R1 | R2>;
}
declare const p: PL<number>;
const q: PL<string> = p.then(v => String(v));
const bad: PL<boolean> = p.then(v => String(v)); // error 2322
export {};
