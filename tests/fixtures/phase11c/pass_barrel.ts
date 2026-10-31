import type { Data, Unwrap, ElementOf } from "./lib_barrel";
export const a: Data<{ data: number }> = 1;
export const b: Unwrap<Promise<boolean>> = true;
export const c: ElementOf<string[]> = "s";
