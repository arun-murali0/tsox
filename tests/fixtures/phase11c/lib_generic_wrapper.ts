import type { Unwrap, ElementOf } from "./lib_conditional";
export type UnwrapAll<T> = Unwrap<T>[];
export type FirstOf<T> = ElementOf<T>;
export type Boxed<T> = { inner: Unwrap<T> };
