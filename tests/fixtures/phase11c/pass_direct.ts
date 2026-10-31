import type { IsString, Unwrap, ElementOf, HasId } from "./lib_conditional";
export const a: Unwrap<Promise<string>> = "hello";
export const b: Unwrap<number> = 1;
export const c: ElementOf<number[]> = 2;
export const d: HasId<{ id: "x" }> = "x";
export const e: IsString<string> = true;
export const f: IsString<number> = false;
