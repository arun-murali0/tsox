import type { UnwrapAll, FirstOf, Boxed } from "./lib_generic_wrapper";
export const a: UnwrapAll<Promise<string>> = ["s"];
export const b: FirstOf<number[]> = 1;
export const c: Boxed<Promise<boolean>> = { inner: true };
