import type { IsString, Unwrap, ElementOf } from "./lib_conditional";
export const a: Unwrap<Promise<string>> = 1;
export const b: ElementOf<number[]> = "s";
export const c: IsString<string> = false;
