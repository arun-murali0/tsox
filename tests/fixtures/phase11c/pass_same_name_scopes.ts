import type { ExtractValue, ExtractArrayValue } from "./lib_conditional";
export const a: ExtractValue<{ value: string }> = "s";
export const b: ExtractArrayValue<number[]> = 1;
export const c: ExtractValue<{ value: number[] }> = [1];
