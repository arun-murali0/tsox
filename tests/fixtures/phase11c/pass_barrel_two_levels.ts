import type { Data, Unwrap, Value } from "./lib_barrel2";
export const a: Data<{ data: number }> = 1;
export const b: Unwrap<Promise<string>> = "s";
export const c: Value<{ value: boolean }> = true;
