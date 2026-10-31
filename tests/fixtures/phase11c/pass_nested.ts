import type { DeepUnwrap } from "./lib_conditional";
export const a: DeepUnwrap<Promise<string>> = "s";
export const b: DeepUnwrap<Promise<Promise<number>>> = 2;
export const c: DeepUnwrap<boolean> = true;
