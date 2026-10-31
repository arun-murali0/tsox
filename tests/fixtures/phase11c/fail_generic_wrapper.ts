import type { UnwrapAll, FirstOf, Boxed } from "./lib_generic_wrapper";
export const a: UnwrapAll<Promise<string>> = [1];
export const b: FirstOf<number[]> = "s";
export const c: Boxed<Promise<boolean>> = { inner: 1 };
