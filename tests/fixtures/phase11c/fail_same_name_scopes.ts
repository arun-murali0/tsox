import type { ExtractValue, ExtractArrayValue } from "./lib_conditional";
export const a: ExtractValue<{ value: string }> = 1;
export const b: ExtractArrayValue<number[]> = "s";
