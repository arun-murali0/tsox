import type { DeepUnwrap } from "./lib_conditional";
export const a: DeepUnwrap<Promise<Promise<number>>> = "s";
export const b: DeepUnwrap<boolean> = 1;
export const c: DeepUnwrap<Promise<string>> = 1;
